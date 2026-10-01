import { describe, expect, it } from "vitest";

import type { ScheduleSlot } from "@legacy/bindings/ScheduleSlot";
import type { SpecialRecording } from "@legacy/bindings/SpecialRecording";

import { mapLegacyBlob } from "@lib/migrate-legacy-settings-core";
import {
  checkSpecial,
  deviceDisplayName,
  isoDate,
  SAME_AS_USUAL,
  slotDay,
  slotRows,
  specialDeviceFact,
  specialDeviceId,
  specialDeviceOptions,
  specialRows,
  testWakeWord,
  wakeArmWord,
  wakeWord,
  type TestWakeOutcome,
  type WakeArmResult,
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

// ── Spesialopptakets egen lydenhet ──────────────────────────────────────────

/** Enhetslista slik `toDeviceOptions` gir den: ASIO med prefiks. */
const DEVICES = [
  { id: "asio::Focusrite USB ASIO", name: "Focusrite USB ASIO" },
  { id: "Behringer X32", name: "Behringer X32" },
  { id: "Rode NT-USB", name: "Rode NT-USB" },
];

describe("a special's own audio device", () => {
  it("«Samme som vanlig opptak» is null, never an empty string", () => {
    expect(specialDeviceId(SAME_AS_USUAL)).toBeNull();
    expect(specialDeviceId("   ")).toBeNull();
    expect(specialDeviceId(null)).toBeNull();
    expect(specialDeviceId(undefined)).toBeNull();
    expect(specialDeviceId("Rode NT-USB")).toBe("Rode NT-USB");
  });

  it("adds with the chosen device — and with null when none was chosen", () => {
    const draft = {
      name: "Bryllup",
      date: "2026-12-24",
      start: "16:00",
      minutes: 60,
    };
    expect(withSpecial([], draft, "Gudstjeneste")[0].deviceId).toBeNull();
    expect(
      withSpecial([], { ...draft, deviceId: SAME_AS_USUAL }, "Gudstjeneste")[0]
        .deviceId,
    ).toBeNull();
    expect(
      withSpecial(
        [],
        { ...draft, deviceId: "asio::Focusrite USB ASIO" },
        "Gudstjeneste",
      )[0].deviceId,
    ).toBe("asio::Focusrite USB ASIO");
  });

  it("round-trips UI → sanitize → core JSON → UI", () => {
    // UI: two specials, one on its own device, one on the usual one.
    const ui = withSpecial(
      withSpecial(
        [],
        {
          name: "Bryllup",
          date: "2099-06-20",
          start: "14:00",
          minutes: 90,
          deviceId: "asio::Focusrite USB ASIO",
        },
        "Gudstjeneste",
      ),
      { name: "Konsert", date: "2099-06-21", start: "19:00", minutes: 90 },
      "Gudstjeneste",
    );

    // sanitize: the one-shot import path must not drop the device.
    const sanitized = mapLegacyBlob(JSON.stringify({ specialRecordings: ui }))!
      .specialRecordings as SpecialRecording[];
    expect(sanitized).toEqual(ui);

    // core JSON: the exact `SpecialRecording` binding shape — `deviceId`,
    // camelCase, `null` for "same as usual" — survives a serialise/parse.
    const core = JSON.parse(JSON.stringify(sanitized)) as SpecialRecording[];
    expect(Object.keys(core[0]).sort()).toEqual(
      ["date", "deviceId", "id", "name", "start", "stop"].sort(),
    );
    expect(core[0].deviceId).toBe("asio::Focusrite USB ASIO");
    expect(core[1].deviceId).toBeNull();

    // UI again: the rows say which device, by the name on the box.
    const rows = specialRows(core, "2099-01-01");
    expect(
      rows.map((r) => specialDeviceFact(r.value.deviceId, DEVICES)),
    ).toEqual([{ name: "Focusrite USB ASIO", present: true }, null]);
  });

  it("sanitize turns a blank or non-string device into null", () => {
    const out = mapLegacyBlob(
      JSON.stringify({
        specialRecordings: [
          { date: "2099-01-01", name: "a", deviceId: "  " },
          { date: "2099-01-02", name: "b", deviceId: 42 },
          { date: "2099-01-03", name: "c" },
        ],
      }),
    )!.specialRecordings as SpecialRecording[];
    expect(out.map((r) => r.deviceId)).toEqual([null, null, null]);
  });

  it("offers each device once, and keeps a chosen one that is unplugged", () => {
    expect(specialDeviceOptions(DEVICES, null)).toEqual([
      { value: "asio::Focusrite USB ASIO", label: "Focusrite USB ASIO" },
      { value: "Behringer X32", label: "Behringer X32" },
      { value: "Rode NT-USB", label: "Rode NT-USB" },
    ]);
    // Two identical USB cards: same name, same id → one option.
    expect(
      specialDeviceOptions(
        [
          { id: "USB Audio CODEC", name: "USB Audio CODEC" },
          { id: "USB Audio CODEC", name: "USB Audio CODEC" },
        ],
        null,
      ),
    ).toHaveLength(1);
    // Chosen, then unplugged: the box must still show what will be saved.
    expect(
      specialDeviceOptions([], "asio::Zoom H6").map((o) => o.label),
    ).toEqual(["Zoom H6"]);
    // Not read yet → nothing to offer but what is chosen.
    expect(specialDeviceOptions(null, null)).toEqual([]);
  });

  it("says when a special's device is not connected — and not before it knows", () => {
    expect(specialDeviceFact(null, DEVICES)).toBeNull();
    expect(specialDeviceFact("Rode NT-USB", DEVICES)).toEqual({
      name: "Rode NT-USB",
      present: true,
    });
    expect(specialDeviceFact("asio::Zoom H6", DEVICES)).toEqual({
      name: "Zoom H6",
      present: false,
    });
    expect(specialDeviceFact("asio::Zoom H6", null)).toEqual({
      name: "Zoom H6",
      present: null,
    });
    expect(deviceDisplayName("asio::Focusrite USB ASIO")).toBe(
      "Focusrite USB ASIO",
    );
    expect(deviceDisplayName("Rode NT-USB")).toBe("Rode NT-USB");
  });
});
