import { describe, expect, it } from "vitest";

import type { ScheduleSlot } from "@legacy/bindings/ScheduleSlot";

import {
  isoDate,
  slotDay,
  slotRows,
  testWakeWord,
  wakeArmWord,
  wakeWord,
  type TestWakeOutcome,
  type WakeArmResult,
  withoutIndex,
  withSlot,
} from "./slots-core";

function slot(day: number, start: string, stop: string): ScheduleSlot {
  return { days: [day], start, stop, max: null };
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

describe("wakeArmWord", () => {
  it.each([
    ["ikke forsøkt", null, "idle"],
    ["registrert to vekkinger", { ok: true, reason: null, count: 2 }, "ok"],
    ["trenger admin", { ok: false, reason: "permission" }, "needsAdmin"],
    ["bryteren er av", { ok: false, reason: "disabled" }, "disabled"],
    ["maskinen kan ikke", { ok: false, reason: "unsupported" }, "unsupported"],
    ["brukeren avbrøt", { ok: false, reason: "cancelled" }, "cancelled"],
    ["generisk feil", { ok: false, reason: "error" }, "failed"],
    // En grunn vi ikke har et ord for er fortsatt en FEIL. Å la den falle til
    // «det gikk bra» ville vært den ene løgnen denne raden finnes for å slutte
    // med: bryteren sa «på», og ingenting var armet.
    ["ukjent grunn", { ok: false, reason: "noe-nytt-fra-rust" }, "failed"],
    ["ingen grunn i det hele tatt", { ok: false, reason: null }, "failed"],
    // `ok: true` er ikke ETT svar. En vellykket runde som armet NULL er en
    // knapp som ser ut som den virket, og bakenden sier hvilken ingenting det
    // er (`WakeIdleReason`).
    [
      "lyktes, men «Ta opp automatisk» er av",
      { ok: true, reason: null, count: 0, idleReason: "autoRecordOff" },
      "autoRecordOff",
    ],
    [
      "lyktes, men ingenting ligger innenfor horisonten",
      { ok: true, reason: null, count: 0, idleReason: "nothingUpcoming" },
      "nothingUpcoming",
    ],
    // Feltet er `serde(default)`, så en eldre payload kan mangle det. Da
    // faller vi til den generelle av de to — som er sann i begge tilfellene —
    // og aldri til «registrert».
    [
      "lyktes med null, uten en grunn",
      { ok: true, reason: null, count: 0 },
      "nothingUpcoming",
    ],
    [
      "lyktes uten et tall i det hele tatt",
      { ok: true, reason: null },
      "nothingUpcoming",
    ],
  ] as Array<[string, WakeArmResult | null, string]>)(
    "%s",
    (_name, result, word) => {
      expect(wakeArmWord(result)).toBe(word);
    },
  );
});

describe("testWakeWord", () => {
  it.each([
    ["ikke forsøkt ennå", null, "idle"],
    // Ulikt `wakeArmWord`: en vellykket test armer alltid nøyaktig én
    // vekking, to minutter fram — det finnes ikke et «ok, men null»-utfall
    // her, så `count`/`idleReason` er ikke en del av `TestWakeOutcome`.
    ["planlagt", { ok: true, reason: null }, "scheduled"],
    // De fire feilordene er DE SAMME som `wakeArmWord` bruker for de samme
    // `WakeErrorReason`-strengene — testen speiler den tabellen med vilje.
    ["trenger admin", { ok: false, reason: "permission" }, "needsAdmin"],
    ["maskinen kan ikke", { ok: false, reason: "unsupported" }, "unsupported"],
    ["brukeren avbrøt", { ok: false, reason: "cancelled" }, "cancelled"],
    ["generisk feil", { ok: false, reason: "error" }, "failed"],
    ["ukjent grunn", { ok: false, reason: "noe-nytt-fra-rust" }, "failed"],
    ["ingen grunn i det hele tatt", { ok: false, reason: null }, "failed"],
  ] as Array<[string, TestWakeOutcome | null, string]>)(
    "%s",
    (_name, result, word) => {
      expect(testWakeWord(result)).toBe(word);
    },
  );
});
