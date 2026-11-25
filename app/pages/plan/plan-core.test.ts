import { describe, expect, it } from "vitest";

import type { ScheduleSlot } from "@legacy/bindings/ScheduleSlot";

import {
  appendPlacement,
  ceilSnap,
  createPlacement,
  dayFromX,
  dayHeightPx,
  deleteOccurrence,
  endOf,
  findConflict,
  fitDuration,
  floorSnap,
  HOUR_PX,
  initialTopMinutes,
  intervalsOverlap,
  keyPlacement,
  minutesToPx,
  missedKeysThisWeek,
  movePlacement,
  nextKey,
  occurrenceKey,
  occurrencesOf,
  placementsOverlap,
  pxToMinutes,
  rangeLabel,
  replaceOccurrence,
  resizeEnd,
  resizeStart,
  segmentsOf,
  slotTimes,
  snapMinutes,
  suggestPlacement,
  weekdayOf,
  weekStart,
  type Placement,
} from "./plan-core";

const slot = (
  days: number[],
  start: string,
  stop: string,
  max: number | null = null,
): ScheduleSlot => ({ days, start, stop, max });

const SUNDAY_SERVICE = slot([6], "11:00", "12:30");

describe("piksler og tid", () => {
  it("minutter ↔ piksler er hverandres inverse", () => {
    for (const m of [0, 15, 90, 660, 1439]) {
      expect(pxToMinutes(minutesToPx(m))).toBeCloseTo(m, 6);
    }
    expect(minutesToPx(60)).toBe(HOUR_PX);
    expect(dayHeightPx()).toBe(24 * HOUR_PX);
  });

  it("snapper til nærmeste kvarter, og runder ned/opp på forespørsel", () => {
    expect(snapMinutes(7)).toBe(0);
    expect(snapMinutes(8)).toBe(15);
    expect(snapMinutes(22)).toBe(15);
    expect(snapMinutes(23)).toBe(30);
    expect(floorSnap(29)).toBe(15);
    expect(ceilSnap(31)).toBe(45);
    expect(ceilSnap(30)).toBe(30);
  });

  it("finner kolonnen fra x, og holder seg innenfor mandag–søndag", () => {
    expect(dayFromX(0, 700)).toBe(0);
    expect(dayFromX(99, 700)).toBe(0);
    expect(dayFromX(100, 700)).toBe(1);
    expect(dayFromX(699, 700)).toBe(6);
    expect(dayFromX(-50, 700)).toBe(0);
    expect(dayFromX(5000, 700)).toBe(6);
    expect(dayFromX(10, 0)).toBe(0);
  });

  it("mandag er 0 og søndag er 6", () => {
    expect(weekdayOf(new Date(2026, 10, 23))).toBe(0); // man 23. nov 2026
    expect(weekdayOf(new Date(2026, 10, 29))).toBe(6);
  });
});

describe("slot → opptredener → segmenter", () => {
  it("én slot, én dag: ett helt segment", () => {
    const occs = occurrencesOf([SUNDAY_SERVICE]);
    expect(occs).toEqual([{ index: 0, day: 6, start: 660, duration: 90 }]);
    const segs = segmentsOf(occs[0]);
    expect(segs).toHaveLength(1);
    expect(segs[0]).toMatchObject({
      day: 6,
      from: 660,
      to: 750,
      part: "whole",
    });
  });

  it("en slot med flere dager gir én opptreden per dag, og dupliserte dager telles én gang", () => {
    const occs = occurrencesOf([slot([2, 6, 6], "19:00", "20:00")]);
    expect(occs.map((o) => o.day)).toEqual([2, 6]);
    expect(occs.every((o) => o.index === 0)).toBe(true);
  });

  it("hopper over ugyldige og degenererte slots uten å miste indeksene", () => {
    const occs = occurrencesOf([
      slot([0], "xx", "12:00"),
      slot([1], "10:00", "10:00"),
      slot([9], "10:00", "11:00"),
      slot([3], "10:00", "11:00"),
    ]);
    expect(occs).toEqual([{ index: 3, day: 3, start: 600, duration: 60 }]);
  });

  it("midnattskryssende slot deles i hode og hale på neste dag", () => {
    const [occ] = occurrencesOf([slot([4], "23:00", "01:00")]);
    expect(occ.duration).toBe(120);
    const [head, tail] = segmentsOf(occ);
    expect(head).toMatchObject({ day: 4, from: 1380, to: 1440, part: "head" });
    expect(tail).toMatchObject({ day: 5, from: 0, to: 60, part: "tail" });
    expect(head.key).not.toBe(tail.key);
  });

  it("søndag kveld renner over til mandag", () => {
    const [occ] = occurrencesOf([slot([6], "23:30", "00:30")]);
    const [, tail] = segmentsOf(occ);
    expect(tail.day).toBe(0);
    expect(tail.to).toBe(30);
  });

  it("slutt akkurat ved midnatt krysser ikke", () => {
    const [occ] = occurrencesOf([slot([1], "22:00", "00:00")]);
    expect(occ.duration).toBe(120);
    expect(segmentsOf(occ)).toHaveLength(1);
    expect(segmentsOf(occ)[0].to).toBe(1440);
  });

  it("etikett og slot-felter, også over midnatt", () => {
    expect(rangeLabel({ start: 660, duration: 90 })).toBe("11:00–12:30");
    expect(rangeLabel({ start: 1380, duration: 120 })).toBe("23:00–01:00");
    expect(slotTimes({ day: 0, start: 1380, duration: 120 })).toEqual({
      start: "23:00",
      stop: "01:00",
    });
  });
});

describe("overlapp", () => {
  it("vegg-i-vegg er lov, ett minutt inn i hverandre er ikke", () => {
    expect(intervalsOverlap(0, 60, 60, 60)).toBe(false);
    expect(intervalsOverlap(0, 61, 60, 60)).toBe(true);
    expect(intervalsOverlap(30, 10, 0, 100)).toBe(true);
  });

  it("uka er en sirkel: søndag 23:00–01:00 kolliderer med mandag 00:30", () => {
    const sun: Placement = { day: 6, start: 1380, duration: 120 };
    expect(placementsOverlap(sun, { day: 0, start: 30, duration: 30 })).toBe(
      true,
    );
    expect(placementsOverlap(sun, { day: 0, start: 60, duration: 30 })).toBe(
      false,
    );
  });

  it("mandag tidlig kolliderer med slutten av søndag kveld, også omvendt", () => {
    const mon: Placement = { day: 0, start: 0, duration: 30 };
    expect(placementsOverlap(mon, { day: 6, start: 1410, duration: 60 })).toBe(
      true,
    );
  });

  it("samme klokkeslett på ulike dager kolliderer ikke", () => {
    expect(
      placementsOverlap(
        { day: 1, start: 600, duration: 60 },
        { day: 2, start: 600, duration: 60 },
      ),
    ).toBe(false);
  });

  it("midnattskryssende slot er i veien for neste dags morgen", () => {
    const occs = occurrencesOf([slot([2], "23:00", "01:00")]);
    expect(
      findConflict(occs, { day: 3, start: 30, duration: 30 }),
    ).toMatchObject({ index: 0 });
    expect(findConflict(occs, { day: 3, start: 60, duration: 30 })).toBeNull();
  });

  it("opptredenen som flyttes kolliderer ikke med seg selv", () => {
    const occs = occurrencesOf([SUNDAY_SERVICE]);
    const moved = { day: 6 as const, start: 675, duration: 90 };
    expect(findConflict(occs, moved)).not.toBeNull();
    expect(findConflict(occs, moved, { index: 0, day: 6 })).toBeNull();
  });

  it("en slot med flere dager: dag A og dag B er hver sin opptreden", () => {
    const occs = occurrencesOf([slot([1, 3], "10:00", "11:00")]);
    // Flytter tirsdag-opptredenen oppå onsdagens.
    expect(
      findConflict(
        occs,
        { day: 3, start: 600, duration: 60 },
        { index: 0, day: 1 },
      ),
    ).toMatchObject({ day: 3 });
  });

  it("fitDuration kutter til det som får plass, og gir opp under 15 min", () => {
    const occs = occurrencesOf([slot([6], "12:00", "13:00")]);
    // 11:00 + 90 min går 30 min inn i neste → krymper til 60.
    expect(fitDuration(occs, { day: 6, start: 660, duration: 90 })).toEqual({
      day: 6,
      start: 660,
      duration: 60,
    });
    // 11:50 har ikke 15 min før 12:00.
    expect(fitDuration(occs, { day: 6, start: 710, duration: 90 })).toBeNull();
    // 11:45 har akkurat 15.
    expect(fitDuration(occs, { day: 6, start: 705, duration: 90 })).toEqual({
      day: 6,
      start: 705,
      duration: 15,
    });
  });

  it("suggestPlacement tar standardtiden når den er ledig, ellers neste ledige", () => {
    const pref: Placement = { day: 6, start: 660, duration: 90 };
    expect(suggestPlacement([], pref)).toEqual(pref);
    const occs = occurrencesOf([SUNDAY_SERVICE]);
    const next = suggestPlacement(occs, pref);
    expect(next).not.toBeNull();
    expect(findConflict(occs, next as Placement)).toBeNull();
    expect(next).toEqual({ day: 0, start: 0, duration: 90 });
  });
});

describe("opprette ved gest", () => {
  it("et trykk gir 90 min fra kvarteret pekeren står i", () => {
    expect(createPlacement(2, 667, 667, true)).toEqual({
      day: 2,
      start: 660,
      duration: 90,
    });
  });

  it("et trykk sent på kvelden kan krysse midnatt", () => {
    const p = createPlacement(6, 1430, 1430, true);
    expect(p.start).toBe(1425);
    expect(crosses(p)).toBe(true);
  });

  it("en draing gir nøyaktig det som ble dratt, rundet ut til kvarter", () => {
    expect(createPlacement(1, 601, 689, false)).toEqual({
      day: 1,
      start: 600,
      duration: 90,
    });
  });

  it("draing oppover gir samme resultat som nedover", () => {
    expect(createPlacement(1, 689, 601, false)).toEqual(
      createPlacement(1, 601, 689, false),
    );
  });

  it("en bitteliten draing blir minst 15 min", () => {
    expect(createPlacement(1, 601, 603, false).duration).toBe(15);
  });

  it("draing forbi bunnen stopper ved midnatt", () => {
    const p = createPlacement(1, 1400, 5000, false);
    expect(endOf(p)).toBe(1440);
  });
});

function crosses(p: Placement): boolean {
  return endOf(p) > 1440;
}

describe("flytte", () => {
  const base: Placement = { day: 6, start: 660, duration: 90 };

  it("flytter og snapper minuttene", () => {
    expect(movePlacement(base, 0, 22)).toMatchObject({ start: 675 });
    expect(movePlacement(base, 0, -37)).toMatchObject({ start: 630 });
  });

  it("flytter på tvers av dager, og blir i uka", () => {
    expect(movePlacement(base, -3, 0).day).toBe(3);
    expect(movePlacement(base, 5, 0).day).toBe(6);
    expect(movePlacement({ ...base, day: 0 }, -2, 0).day).toBe(0);
  });

  it("varigheten er uendret", () => {
    expect(movePlacement(base, 2, 300).duration).toBe(90);
  });

  it("starten holdes innenfor dagen", () => {
    expect(movePlacement(base, 0, -5000).start).toBe(0);
    expect(movePlacement(base, 0, 5000).start).toBe(1425);
  });

  it("en blokk flyttet sent på kvelden krysser midnatt og beholder varigheten", () => {
    const p = movePlacement(base, 0, 5000);
    expect(p).toEqual({ day: 6, start: 1425, duration: 90 });
    expect(segmentsOf({ index: 0, ...p })).toHaveLength(2);
  });
});

describe("strekke", () => {
  const base: Placement = { day: 6, start: 660, duration: 90 };

  it("bunnkanten endrer slutten og snapper", () => {
    expect(resizeEnd(base, 765)).toEqual({ ...base, duration: 105 });
    expect(resizeEnd(base, 767)).toEqual({ ...base, duration: 105 });
  });

  it("toppkanten endrer starten og lar slutten stå", () => {
    const next = resizeStart(base, 630);
    expect(next.start).toBe(630);
    expect(endOf(next)).toBe(750);
  });

  it("minimum er 15 min — fra begge kanter", () => {
    expect(resizeEnd(base, 0).duration).toBe(15);
    expect(resizeEnd(base, 661).duration).toBe(15);
    const s = resizeStart(base, 5000);
    expect(s.duration).toBe(15);
    expect(endOf(s)).toBe(750);
  });

  it("toppkanten kan ikke dras forbi midnatt (start ≥ 0)", () => {
    expect(resizeStart(base, -300).start).toBe(0);
  });

  it("slutten stopper ved midnatt når musa ber om det, men kan krysse via limit", () => {
    expect(resizeEnd(base, 9999, 1440).duration).toBe(1440 - 660);
    const crossed = resizeEnd(base, 1500);
    expect(endOf(crossed)).toBe(1500);
  });

  it("varigheten overstiger aldri 23:45", () => {
    const long = resizeEnd({ day: 0, start: 0, duration: 60 }, 99999);
    expect(long.duration).toBe(1425);
    const early = resizeStart({ day: 0, start: 600, duration: 1000 }, 0);
    expect(early.duration).toBeLessThanOrEqual(1425);
  });

  it("hale-kanten (slutt over midnatt) kan strekkes og krympes", () => {
    const crossing: Placement = { day: 2, start: 1380, duration: 120 };
    expect(endOf(resizeEnd(crossing, 1560))).toBe(1560);
    // Krymper til før midnatt: blir en vanlig blokk.
    const shrunk = resizeEnd(crossing, 1425);
    expect(shrunk.duration).toBe(45);
    expect(crosses(shrunk)).toBe(false);
  });
});

describe("tastaturet", () => {
  const base: Placement = { day: 3, start: 600, duration: 60 };

  it("opp/ned flytter ett kvarter, venstre/høyre én dag", () => {
    expect(keyPlacement(base, "up").start).toBe(585);
    expect(keyPlacement(base, "down").start).toBe(615);
    expect(keyPlacement(base, "left").day).toBe(2);
    expect(keyPlacement(base, "right").day).toBe(4);
  });

  it("skift + opp/ned endrer lengden med ett kvarter", () => {
    expect(keyPlacement(base, "grow").duration).toBe(75);
    expect(keyPlacement(base, "shrink").duration).toBe(45);
  });

  it("stopper ved kantene i stedet for å gå rundt", () => {
    expect(keyPlacement({ ...base, day: 0 }, "left").day).toBe(0);
    expect(keyPlacement({ ...base, day: 6 }, "right").day).toBe(6);
    expect(keyPlacement({ ...base, start: 0 }, "up").start).toBe(0);
    expect(keyPlacement({ ...base, start: 1425 }, "down").start).toBe(1425);
    expect(keyPlacement({ ...base, duration: 15 }, "shrink").duration).toBe(15);
  });

  it("forlenger forbi midnatt", () => {
    const late: Placement = { day: 1, start: 1380, duration: 60 };
    const longer = keyPlacement(late, "grow");
    expect(endOf(longer)).toBe(1455);
    expect(crosses(longer)).toBe(true);
  });
});

describe("skriving til slot-listen", () => {
  it("replaceOccurrence skriver på plass og beholder max", () => {
    const slots = [
      slot([6], "11:00", "12:30", 100),
      slot([2], "19:00", "20:00"),
    ];
    const out = replaceOccurrence(
      slots,
      { index: 0, day: 6 },
      { day: 0, start: 540, duration: 60 },
    );
    expect(out.slots[0]).toEqual({
      days: [0],
      start: "09:00",
      stop: "10:00",
      max: 100,
    });
    expect(out.slots[1]).toEqual(slots[1]);
    expect(out.ref).toEqual({ index: 0, day: 0 });
    // Inndata er urørt.
    expect(slots[0].days).toEqual([6]);
  });

  it("skriver slutt etter midnatt som en stopp som er mindre enn starten", () => {
    const out = replaceOccurrence(
      [SUNDAY_SERVICE],
      { index: 0, day: 6 },
      { day: 6, start: 1380, duration: 120 },
    );
    expect(out.slots[0]).toMatchObject({ start: "23:00", stop: "01:00" });
  });

  it("en slot med flere dager deles: bare den flyttede dagen forlater slotten", () => {
    const slots = [slot([1, 3], "10:00", "11:00", 70)];
    const out = replaceOccurrence(
      slots,
      { index: 0, day: 3 },
      { day: 4, start: 720, duration: 30 },
    );
    expect(out.slots).toEqual([
      { days: [1], start: "10:00", stop: "11:00", max: 70 },
      { days: [4], start: "12:00", stop: "12:30", max: 70 },
    ]);
    expect(out.ref).toEqual({ index: 1, day: 4 });
  });

  it("deleteOccurrence fjerner slotten, eller bare dagen", () => {
    const slots = [slot([1, 3], "10:00", "11:00"), slot([6], "11:00", "12:30")];
    expect(deleteOccurrence(slots, { index: 0, day: 1 })).toEqual([
      slot([3], "10:00", "11:00"),
      slots[1],
    ]);
    expect(deleteOccurrence(slots, { index: 1, day: 6 })).toEqual([slots[0]]);
  });

  it("peker referansen på noe som ikke finnes, skjer ingenting", () => {
    const slots = [SUNDAY_SERVICE];
    expect(deleteOccurrence(slots, { index: 3, day: 6 })).toEqual(slots);
    expect(deleteOccurrence(slots, { index: 0, day: 2 })).toEqual(slots);
    expect(
      replaceOccurrence(
        slots,
        { index: 0, day: 2 },
        { day: 1, start: 0, duration: 15 },
      ).slots,
    ).toEqual(slots);
  });

  it("appendPlacement legger til sist, som withSlot", () => {
    const out = appendPlacement([SUNDAY_SERVICE], {
      day: 2,
      start: 1140,
      duration: 90,
    });
    expect(out.slots).toHaveLength(2);
    expect(out.slots[1]).toEqual({
      days: [2],
      start: "19:00",
      stop: "20:30",
      max: null,
    });
    expect(out.ref).toEqual({ index: 1, day: 2 });
  });
});

describe("neste, savnet og nå", () => {
  it("nextKey gir dag og klokkeslett fra planleggerens lokal-ISO", () => {
    // søndag 29. nov 2026 11:00
    expect(nextKey("2026-11-29T11:00:00")).toBe(occurrenceKey(6, 660));
    expect(nextKey(null)).toBeNull();
    expect(nextKey("ikke en dato")).toBeNull();
  });

  it("weekStart er mandag 00:00", () => {
    const start = weekStart(new Date(2026, 10, 27, 15, 30)); // fre 27. nov
    expect(start.getFullYear()).toBe(2026);
    expect(start.getMonth()).toBe(10);
    expect(start.getDate()).toBe(23);
    expect(start.getHours()).toBe(0);
    const sunday = weekStart(new Date(2026, 10, 29, 23, 59));
    expect(sunday.getDate()).toBe(23);
  });

  it("savnede opptak teller bare for denne uka", () => {
    const now = new Date(2026, 10, 27, 12, 0); // fredag
    const keys = missedKeysThisWeek(
      [
        { at: "2026-11-24T19:00:00" }, // tirsdag denne uka
        { at: "2026-11-22T11:00:00" }, // søndag FORRIGE uke
        { at: "2026-11-30T11:00:00" }, // mandag NESTE uke
        { at: "søppel" },
      ],
      now,
    );
    expect([...keys]).toEqual([occurrenceKey(1, 1140)]);
  });

  it("rutenettet åpner på 06:00, men viser en tidlig eller sen tid", () => {
    expect(initialTopMinutes([])).toBe(360);
    expect(initialTopMinutes(occurrencesOf([SUNDAY_SERVICE]))).toBe(360);
    expect(
      initialTopMinutes(occurrencesOf([slot([1], "02:30", "03:30")])),
    ).toBe(60);
    expect(
      initialTopMinutes(occurrencesOf([slot([1], "00:30", "01:30")])),
    ).toBe(0);
    // En sen tid trekker toppen ned, men aldri så langt at døgnet tar slutt.
    expect(
      initialTopMinutes(occurrencesOf([slot([1], "21:00", "22:00")])),
    ).toBe(900);
    expect(
      initialTopMinutes(occurrencesOf([slot([1], "23:00", "23:30")])),
    ).toBe(900);
  });

  it("halen av en tid over midnatt trekker ikke visningen opp til natta", () => {
    expect(
      initialTopMinutes(
        occurrencesOf([slot([1], "23:00", "01:00"), SUNDAY_SERVICE]),
      ),
    ).toBe(360);
  });
});
