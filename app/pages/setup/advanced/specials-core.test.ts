import { describe, expect, it } from "vitest";

import type { ScheduleSlot } from "@legacy/bindings/ScheduleSlot";
import type { SpecialRecording } from "@legacy/bindings/SpecialRecording";

import {
  checkSpecial,
  isoDate,
  slotDay,
  slotRows,
  specialRows,
  wakeWord,
  withoutIndex,
  withSlot,
  withSpecial,
} from "./specials-core";

function slot(day: number, start: string, stop: string): ScheduleSlot {
  return { days: [day], start, stop, max: null };
}

function special(
  date: string,
  name: string,
  start = "19:00",
): SpecialRecording {
  return { id: null, date, name, start, stop: "21:00", deviceId: null };
}

describe("slot list", () => {
  it("keeps the stored order, so the level-1 time stays first", () => {
    const slots = [slot(6, "11:00", "12:30"), slot(2, "19:00", "20:00")];
    expect(slotRows(slots).map((r) => r.index)).toEqual([0, 1]);
    expect(slotRows(slots)[0].value.start).toBe("11:00");
  });

  it("adds a time with the duration turned into a stop", () => {
    const out = withSlot([], 6, "23:30", 90);
    expect(out).toHaveLength(1);
    // Past midnight, as `stopFor` handles it.
    expect(out[0]).toEqual({
      days: [6],
      start: "23:30",
      stop: "01:00",
      max: null,
    });
  });

  it("removes exactly the stored index and nothing else", () => {
    const slots = [slot(6, "11:00", "12:00"), slot(2, "19:00", "20:00")];
    expect(withoutIndex(slots, 1)).toEqual([slots[0]]);
    // An index that is not there leaves the list alone rather than dropping
    // the last row — deleting the wrong time is worse than deleting none.
    expect(withoutIndex(slots, 7)).toEqual(slots);
    expect(withoutIndex(slots, -1)).toEqual(slots);
    expect(withoutIndex(null, 0)).toEqual([]);
  });

  it("reads the first chosen weekday, and says null when there is none", () => {
    expect(slotDay(slot(3, "10:00", "11:00"))).toBe(3);
    expect(
      slotDay({ days: [], start: "10:00", stop: "11:00", max: null }),
    ).toBeNull();
    expect(
      slotDay({ days: [9], start: "10:00", stop: "11:00", max: null }),
    ).toBeNull();
  });
});

describe("specials", () => {
  const list = [
    special("2026-12-24", "Julaften"),
    special("2026-01-01", "Nyttår"),
    special("2026-08-30", "Konsert"),
  ];

  it("shows the future in date order — with the STORED index", () => {
    const rows = specialRows(list, "2026-08-23");
    expect(rows.map((r) => r.value.name)).toEqual(["Konsert", "Julaften"]);
    // The seam this file exists for: the row the user sees SECOND is stored
    // FIRST. Removing by the display position would delete the concert.
    expect(rows.map((r) => r.index)).toEqual([2, 0]);
  });

  it("hides a passed date without deleting it", () => {
    const rows = specialRows(list, "2026-08-23");
    expect(rows.some((r) => r.value.name === "Nyttår")).toBe(false);
    expect(list).toHaveLength(3);
  });

  it("today still counts as future", () => {
    expect(
      specialRows([special("2026-08-23", "I dag")], "2026-08-23"),
    ).toHaveLength(1);
  });

  it("adds with the stop derived and a fallback name", () => {
    const out = withSpecial(
      [],
      { name: "  ", date: "2026-12-24", start: "16:00", minutes: 60 },
      "Gudstjeneste",
    );
    expect(out[0]).toEqual({
      id: null,
      date: "2026-12-24",
      name: "Gudstjeneste",
      start: "16:00",
      stop: "17:00",
      deviceId: null,
    });
  });

  it("refuses a draft the backend could not plan", () => {
    const ok = {
      name: "Konsert",
      date: "2026-12-24",
      start: "19:00",
      minutes: 90,
    };
    expect(checkSpecial(ok, "2026-08-23")).toBeNull();
    expect(checkSpecial({ ...ok, date: "" }, "2026-08-23")).toBe("noDate");
    expect(checkSpecial({ ...ok, start: "19:9" }, "2026-08-23")).toBe(
      "badTime",
    );
    expect(checkSpecial({ ...ok, date: "2026-08-22" }, "2026-08-23")).toBe(
      "past",
    );
  });
});

describe("isoDate", () => {
  it("is LOCAL, not UTC", () => {
    // 1 January at 00:30 local is still 1 January. `toISOString()` would say
    // 31 December anywhere west of Greenwich, i.e. «today» would be yesterday
    // every evening.
    expect(isoDate(new Date(2026, 0, 1, 0, 30))).toBe("2026-01-01");
    expect(isoDate(new Date(2026, 11, 24, 23, 59))).toBe("2026-12-24");
  });
});

describe("wakeWord", () => {
  it("is one sentence, and «not read yet» is not «cannot»", () => {
    expect(wakeWord(null)).toBe("unknown");
    expect(wakeWord({ canWakeFromSleep: false, needsAdmin: false })).toBe(
      "cannot",
    );
    expect(wakeWord({ canWakeFromSleep: true, needsAdmin: false })).toBe("can");
    expect(wakeWord({ canWakeFromSleep: true, needsAdmin: true })).toBe(
      "needsAdmin",
    );
    // needsAdmin on a machine that cannot wake at all is still «cannot» — the
    // password would buy nothing.
    expect(wakeWord({ canWakeFromSleep: false, needsAdmin: true })).toBe(
      "cannot",
    );
  });
});
