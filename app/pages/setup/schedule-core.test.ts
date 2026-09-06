import { describe, expect, it } from "vitest";

import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";

import type { Settings } from "../../state/settings";
import {
  anythingScheduled,
  autoRecordOn,
  DEFAULT_PLAN,
  durationBetween,
  isValidTime,
  minutesOfDay,
  planFromSlots,
  slotsFromPlan,
  stopFor,
  timeOfDay,
} from "./schedule-core";

describe("klokkeslett", () => {
  it.each(["00:00", "9:30", "09:30", "23:59"])("«%s» er et tidspunkt", (v) =>
    expect(isValidTime(v)).toBe(true),
  );

  it.each(["", "9.30", "24:00", "11:60", "elleve"])("«%s» er det ikke", (v) =>
    expect(isValidTime(v)).toBe(false),
  );

  it("minutesOfDay svarer null for noe som ikke er et tidspunkt", () => {
    expect(minutesOfDay("11:00")).toBe(660);
    expect(minutesOfDay("9.30")).toBeNull();
  });

  it("timeOfDay går rundt døgnet i begge retninger", () => {
    expect(timeOfDay(660)).toBe("11:00");
    expect(timeOfDay(1440)).toBe("00:00");
    expect(timeOfDay(1500)).toBe("01:00");
    expect(timeOfDay(-60)).toBe("23:00");
  });
});

describe("varighet ↔ stopptidspunkt", () => {
  it("90 minutter fra 11:00 er 12:30", () => {
    expect(stopFor("11:00", 90)).toBe("12:30");
  });

  it("krysser midnatt uten å bli negativ", () => {
    expect(stopFor("23:30", 90)).toBe("01:00");
    expect(durationBetween("23:30", "01:00")).toBe(90);
  });

  it("regner den andre veien også", () => {
    expect(durationBetween("11:00", "12:30")).toBe(90);
  });

  it("faller tilbake på standarden når tiden er søppel", () => {
    expect(durationBetween("9.30", "12:30")).toBe(DEFAULT_PLAN.minutes);
  });
});

describe("planFromSlots", () => {
  it("tom liste er «av»", () => {
    expect(planFromSlots([])).toBeNull();
    expect(planFromSlots(null)).toBeNull();
    expect(planFromSlots(undefined)).toBeNull();
  });

  it("leser dag, start og VARIGHET ut av den første sloten", () => {
    expect(
      planFromSlots([{ days: [6], start: "11:00", stop: "12:30", max: null }]),
    ).toEqual({ day: 6, start: "11:00", minutes: 90 });
  });

  it("flere dager: den første vises, resten røres ikke", () => {
    expect(
      planFromSlots([
        { days: [2, 6], start: "19:00", stop: "20:00", max: null },
      ]),
    ).toEqual({ day: 2, start: "19:00", minutes: 60 });
  });

  it("en ugyldig lagret starttid ender på standarden, ikke på NaN", () => {
    expect(
      planFromSlots([{ days: [6], start: "elleve", stop: "12:30", max: null }]),
    ).toEqual({
      day: 6,
      start: DEFAULT_PLAN.start,
      minutes: DEFAULT_PLAN.minutes,
    });
  });
});

describe("slotsFromPlan", () => {
  it("skriver dag, start og stopp", () => {
    expect(slotsFromPlan({ day: 6, start: "11:00", minutes: 90 }, [])).toEqual([
      { days: [6], start: "11:00", stop: "12:30", max: null },
    ]);
  });

  it("beholder `max` fra sloten den erstatter", () => {
    // Noen satte en hard grense. Varigheten her er ikke den samme tingen, og
    // en skjerm som ikke viser grensen skal ikke slette den heller.
    const before = [{ days: [6], start: "11:00", stop: "12:00", max: 120 }];
    expect(
      slotsFromPlan({ day: 6, start: "11:00", minutes: 90 }, before),
    ).toEqual([{ days: [6], start: "11:00", stop: "12:30", max: 120 }]);
  });

  it("rører ikke tidspunkter nivå 1 ikke viser", () => {
    const before = [
      { days: [6], start: "11:00", stop: "12:00", max: null },
      { days: [2], start: "19:00", stop: "20:30", max: null },
    ];
    const after = slotsFromPlan(
      { day: 6, start: "10:00", minutes: 60 },
      before,
    );
    expect(after).toHaveLength(2);
    expect(after[1]).toEqual(before[1]);
  });

  it("tur–retur er identitet", () => {
    const plan = { day: 3, start: "18:15", minutes: 45 } as const;
    expect(planFromSlots(slotsFromPlan(plan, []))).toEqual(plan);
  });
});

describe("kommer det til å skje noe av seg selv?", () => {
  const SLOT = { days: [6], start: "11:00", stop: "12:30", max: null };
  const SPECIAL = {
    id: null,
    date: "2026-12-24",
    name: "Julaften",
    start: "16:00",
    stop: "17:00",
    deviceId: null,
  };
  const s = (over: Partial<Settings>): Settings => ({
    ...SETTINGS_DEFAULTS,
    ...over,
  });

  it.each([
    ["ingenting satt opp", {}, false, false],
    ["en fast tid, bryteren på", { slots: [SLOT] }, true, true],
    [
      "en fast tid, bryteren AV — tidene står, men ingenting skjer",
      { slots: [SLOT], autoRecordEnabled: false },
      false,
      false,
    ],
    [
      "bryteren på, men ingen tid — et armert flagg uten en slot er ikke «på»",
      { autoRecordEnabled: true },
      false,
      false,
    ],
    [
      "bare et spesialopptak — det skjer uansett hva bryteren står på",
      { specialRecordings: [SPECIAL] },
      false,
      true,
    ],
    [
      "bryteren AV, men et spesialopptak ligger der",
      { slots: [SLOT], autoRecordEnabled: false, specialRecordings: [SPECIAL] },
      false,
      true,
    ],
  ] as Array<[string, Partial<Settings>, boolean, boolean]>)(
    "%s",
    (_name, over, auto, anything) => {
      expect(autoRecordOn(s(over))).toBe(auto);
      // Funnet: helten leste «(slots ?? []).length > 0» og sa derfor «Alt er
      // klart» med bryteren av. Bakenden gater slots på flagget
      // (`Settings::active_slots`) og lar spesialopptak gå gjennom — dette er
      // det samme uttrykket, på riktig side av skjøten.
      expect(anythingScheduled(s(over))).toBe(anything);
    },
  );
});
