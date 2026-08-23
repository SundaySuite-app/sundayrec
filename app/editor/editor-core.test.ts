import { describe, expect, it } from "vitest";

import {
  dragHandle,
  exactSpan,
  keptSeconds,
  MIN_WINDOW_SEC,
  sermonCutRegions,
  sermonWindow,
  suggestionIsWorthOffering,
  suggestionRange,
  timecode,
  windowToCuts,
} from "./editor-core";
import type { Segment } from "./model";

/** Et opptak med hode, preken, en sang inni, og en hale. 600 sekunder. */
const SEGMENTS: Segment[] = [
  { start: 0, end: 30, duration: 30, label: "Stillhet", type: "silence" },
  { start: 30, end: 180, duration: 150, label: "Tale", type: "speech" },
  { start: 180, end: 210, duration: 30, label: "Musikk", type: "music" },
  { start: 210, end: 420, duration: 210, label: "Preken", type: "sermon" },
  { start: 420, end: 600, duration: 180, label: "Tale", type: "speech" },
];

const DURATION = 600;

describe("forslaget", () => {
  it("er prekenblokkens vindu", () => {
    expect(suggestionRange(SEGMENTS)).toEqual({ start: 210, end: 420 });
  });

  it("finnes ikke når ingen blokk er merket som preken", () => {
    const none = SEGMENTS.map((s) =>
      s.type === "sermon" ? { ...s, type: "speech" } : s,
    );
    expect(suggestionRange(none)).toBeNull();
  });

  it("tilbys bare når det faktisk er noe å trimme", () => {
    expect(suggestionIsWorthOffering({ start: 210, end: 420 }, DURATION)).toBe(
      true,
    );
    // Prekenen ER hele opptaket: «behold bare prekenen» og «behold alt» er da
    // samme handling, og et valg uten forskjell er verre enn ingen kort.
    expect(
      suggestionIsWorthOffering({ start: 0, end: DURATION }, DURATION),
    ).toBe(false);
    // Under et halvsekund i hver ende er avrunding, ikke et kutt.
    expect(
      suggestionIsWorthOffering({ start: 0.2, end: DURATION - 0.2 }, DURATION),
    ).toBe(false);
    expect(suggestionIsWorthOffering(null, DURATION)).toBe(false);
  });
});

describe("prekenvinduet", () => {
  it("er forslaget før «Behold bare prekenen»", () => {
    expect(
      sermonWindow({
        cuts: [],
        duration: DURATION,
        suggestion: { start: 210, end: 420 },
        applied: false,
      }),
    ).toEqual({ start: 210, end: 420 });
  });

  it("er den ytre grensen av det som er igjen etterpå", () => {
    // Hode, hale OG musikk-kuttet inne i prekenen: vinduet er fortsatt
    // 210–420, ikke det første indre kuttet.
    const cuts = [
      { start: 0, end: 210 },
      { start: 300, end: 330 },
      { start: 420, end: DURATION },
    ];
    expect(
      sermonWindow({
        cuts,
        duration: DURATION,
        suggestion: { start: 210, end: 420 },
        applied: true,
      }),
    ).toEqual({ start: 210, end: 420 });
  });

  it("finnes ikke når kuttene dekker hele opptaket", () => {
    expect(
      sermonWindow({
        cuts: [{ start: 0, end: DURATION }],
        duration: DURATION,
        suggestion: null,
        applied: true,
      }),
    ).toBeNull();
  });
});

describe("vinduet tilbake til kutt", () => {
  it("gir ett hode-kutt og ett hale-kutt", () => {
    expect(windowToCuts({ start: 210, end: 420 }, DURATION)).toEqual([
      { start: 0, end: 210 },
      { start: 420, end: DURATION },
    ]);
  });

  it("beholder kuttene som lå INNE i vinduet", () => {
    // Å dra venstre håndtak skal ikke slette musikk-kuttet «Behold bare
    // prekenen» la inn.
    const existing = [
      { start: 0, end: 210 },
      { start: 300, end: 330 },
      { start: 420, end: DURATION },
    ];
    expect(windowToCuts({ start: 240, end: 400 }, DURATION, existing)).toEqual([
      { start: 0, end: 240 },
      { start: 300, end: 330 },
      { start: 400, end: DURATION },
    ]);
  });

  it("dropper kutt i kantene som blir borte i avrunding", () => {
    expect(windowToCuts({ start: 0.2, end: DURATION - 0.2 }, DURATION)).toEqual(
      [],
    );
  });
});

describe("håndtakene", () => {
  const window_ = { start: 210, end: 420 };

  it("kan ikke krysse hverandre", () => {
    expect(dragHandle(window_, "start", 500, DURATION).start).toBe(
      420 - MIN_WINDOW_SEC,
    );
    expect(dragHandle(window_, "end", 100, DURATION).end).toBe(
      210 + MIN_WINDOW_SEC,
    );
  });

  it("kan ikke ut av opptaket", () => {
    expect(dragHandle(window_, "start", -50, DURATION).start).toBe(0);
    expect(dragHandle(window_, "end", DURATION + 50, DURATION).end).toBe(
      DURATION,
    );
  });

  it("flytter bare sin egen side", () => {
    expect(dragHandle(window_, "start", 240, DURATION)).toEqual({
      start: 240,
      end: 420,
    });
    expect(dragHandle(window_, "end", 400, DURATION)).toEqual({
      start: 210,
      end: 400,
    });
  });
});

describe("kuttene rundt prekenen", () => {
  it("fjerner hodet, halen og musikken inni — men ikke stillheten", () => {
    expect(
      sermonCutRegions({ start: 210, end: 420 }, SEGMENTS, DURATION),
    ).toEqual([
      { start: 0, end: 210 },
      { start: 420, end: DURATION },
    ]);
  });

  it("tar en sang som ligger inne i prekenen", () => {
    const withInnerSong: Segment[] = [
      ...SEGMENTS,
      { start: 300, end: 330, duration: 30, label: "Musikk", type: "music" },
    ];
    expect(
      sermonCutRegions({ start: 210, end: 420 }, withInnerSong, DURATION),
    ).toEqual([
      { start: 0, end: 210 },
      { start: 300, end: 330 },
      { start: 420, end: DURATION },
    ]);
  });

  it("lager ingen kutt når prekenen er hele opptaket", () => {
    expect(sermonCutRegions({ start: 0, end: DURATION }, [], DURATION)).toEqual(
      [],
    );
  });
});

describe("resultatet", () => {
  it("teller det som blir igjen", () => {
    expect(keptSeconds([], DURATION)).toBe(DURATION);
    expect(
      keptSeconds(
        [
          { start: 0, end: 210 },
          { start: 420, end: DURATION },
        ],
        DURATION,
      ),
    ).toBe(210);
  });

  it("har tre former, og sekundene er med under en time", () => {
    expect(exactSpan(45)).toEqual({
      kind: "seconds",
      hours: 0,
      minutes: 0,
      seconds: 45,
    });
    expect(exactSpan(1690)).toEqual({
      kind: "minutesSeconds",
      hours: 0,
      minutes: 28,
      seconds: 10,
    });
    expect(exactSpan(3734)).toEqual({
      kind: "hoursMinutes",
      hours: 1,
      minutes: 2,
      seconds: 0,
    });
  });

  it("avrunder FØR den deler, så «1 min 60 s» ikke finnes", () => {
    expect(exactSpan(119.6)).toEqual({
      kind: "minutesSeconds",
      hours: 0,
      minutes: 2,
      seconds: 0,
    });
  });

  it("behandler tull som null", () => {
    expect(exactSpan(Number.NaN).seconds).toBe(0);
    expect(exactSpan(-5).seconds).toBe(0);
  });
});

describe("klokka", () => {
  it("har alltid timetallet med, så tegnene ikke hopper midt i avspillingen", () => {
    expect(timecode(0)).toBe("0:00:00");
    expect(timecode(68)).toBe("0:01:08");
    expect(timecode(3599)).toBe("0:59:59");
    expect(timecode(3734)).toBe("1:02:14");
  });

  it("er null for tull", () => {
    expect(timecode(Number.NaN)).toBe("0:00:00");
    expect(timecode(-1)).toBe("0:00:00");
  });
});
