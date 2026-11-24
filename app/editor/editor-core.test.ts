import { describe, expect, it } from "vitest";

import {
  dragHandle,
  exactSpan,
  keepWindow,
  keptSeconds,
  MIN_WINDOW_SEC,
  timecode,
  windowToCuts,
} from "./editor-core";

/** Et opptak på 600 sekunder. */
const DURATION = 600;

describe("vinduet", () => {
  it("er hele opptaket når ingenting er kuttet", () => {
    expect(keepWindow({ cuts: [], duration: DURATION })).toEqual({
      start: 0,
      end: DURATION,
    });
  });

  it("er den ytre grensen av det som er igjen", () => {
    // Hode, hale OG et kutt inne i utdraget: vinduet er fortsatt 210–420, ikke
    // det første indre kuttet.
    const cuts = [
      { start: 0, end: 210 },
      { start: 300, end: 330 },
      { start: 420, end: DURATION },
    ];
    expect(keepWindow({ cuts, duration: DURATION })).toEqual({
      start: 210,
      end: 420,
    });
  });

  it("finnes ikke når kuttene dekker hele opptaket", () => {
    expect(
      keepWindow({ cuts: [{ start: 0, end: DURATION }], duration: DURATION }),
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
    // Å dra venstre håndtak skal ikke slette kuttet midt i.
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

  it("et vindu som gjøres STØRRE tar ikke det gamle hodet og halen med seg", () => {
    // Feilen: hodet 0–210 ble klemt inn i det nye vinduet og la seg tilbake,
    // så venstre håndtak kunne aldri gå til venstre (og høyre aldri til høyre).
    const existing = [
      { start: 0, end: 210 },
      { start: 300, end: 330 },
      { start: 420, end: DURATION },
    ];
    expect(windowToCuts({ start: 180, end: 450 }, DURATION, existing)).toEqual([
      { start: 0, end: 180 },
      { start: 300, end: 330 },
      { start: 450, end: DURATION },
    ]);
    expect(
      windowToCuts({ start: 0, end: DURATION }, DURATION, existing),
    ).toEqual([{ start: 300, end: 330 }]);
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
