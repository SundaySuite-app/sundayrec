/**
 * Statuslinjens prioritet, som en tabell.
 *
 * Rekkefølgen `rec > lowdisk > nosound > next > ready` er den eneste logikken
 * i modulen, og nøyaktig den slags som ser riktig ut i en komponent og er feil
 * i to av fem tilfeller. Her er hver kombinasjon skrevet ned.
 */

import { describe, expect, it } from "vitest";

import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";

import { soundChosen } from "./devices";
import type { Settings } from "./settings";
import { sourceState, type DeviceFact } from "../pages/record/record-core";
import { formatNextWhen, LOW_DISK_MINUTES, statusLine } from "./status-line";

const SUNDAY_11 = Date.UTC(2026, 7, 30, 9, 0, 0);

/** Alt i orden: kilde valgt, god plass, ingenting planlagt. */
const READY = {
  isRecording: false,
  soundChosen: true,
  roomMinutes: 600,
  nextAtMs: null,
};

describe("statusLine", () => {
  const cases: Array<[string, Parameters<typeof statusLine>[0], string]> = [
    ["alt i orden", READY, "ready"],
    ["ingen kilde valgt", { ...READY, soundChosen: false }, "nosound"],
    ["lite plass", { ...READY, roomMinutes: 30 }, "lowdisk"],
    ["neste opptak er kjent", { ...READY, nextAtMs: SUNDAY_11 }, "next"],
    ["et opptak går", { ...READY, isRecording: true }, "rec"],

    // Prioriteten, par for par — hver rad er én grunn til at rekkefølgen er
    // som den er.
    [
      "opptak slår lite plass",
      { ...READY, isRecording: true, roomMinutes: 5 },
      "rec",
    ],
    [
      "opptak slår manglende kilde",
      { ...READY, isRecording: true, soundChosen: false },
      "rec",
    ],
    [
      "lite plass slår manglende kilde — tom disk stopper opptaket MIDT i det",
      { ...READY, roomMinutes: 5, soundChosen: false },
      "lowdisk",
    ],
    [
      "manglende kilde slår neste opptak",
      { ...READY, soundChosen: false, nextAtMs: SUNDAY_11 },
      "nosound",
    ],
    [
      "neste opptak slår «alt er klart»",
      { ...READY, nextAtMs: SUNDAY_11 },
      "next",
    ],

    // Grensen, begge sider.
    [
      "akkurat på grensen er IKKE lite plass",
      { ...READY, roomMinutes: LOW_DISK_MINUTES },
      "ready",
    ],
    [
      "ett minutt under er det",
      { ...READY, roomMinutes: LOW_DISK_MINUTES - 1 },
      "lowdisk",
    ],

    // Ulest disk er ikke det samme som god plass.
    [
      "disken ikke lest ⇒ ingen påstand om plass",
      { ...READY, roomMinutes: null },
      "ready",
    ],
    [
      "disken ikke lest skjuler ikke en manglende kilde",
      { ...READY, roomMinutes: null, soundChosen: false },
      "nosound",
    ],
  ];

  for (const [what, input, expected] of cases) {
    it(what, () => {
      expect(statusLine(input).kind).toBe(expected);
    });
  }

  it("hver setning har sin egen farge, og rødt betyr bare «det tas opp»", () => {
    expect(statusLine({ ...READY, isRecording: true }).tone).toBe("rec");
    expect(statusLine(READY).tone).toBe("good");
    expect(statusLine({ ...READY, soundChosen: false }).tone).toBe("warn");
    expect(statusLine({ ...READY, roomMinutes: 5 }).tone).toBe("warn");
    expect(statusLine({ ...READY, nextAtMs: SUNDAY_11 }).tone).toBe("neutral");
  });

  it("nøkkelen følger navnet, så katalogen ikke kan komme ut av takt", () => {
    expect(statusLine(READY).key).toBe("app.status.ready");
    expect(statusLine({ ...READY, isRecording: true }).key).toBe(
      "app.status.rec",
    );
  });
});

describe("formatNextWhen", () => {
  it("gir en ukedag og et klokkeslett", () => {
    // Bevisst løs: nøyaktig formatering avhenger av hvilken ICU-versjon
    // node/WebKit er bygget med, og en test som pinner det ville feilet på en
    // node-oppgradering uten at noe var galt.
    const text = formatNextWhen(SUNDAY_11, "no");
    expect(text).toMatch(/\d{1,2}[.:]\d{2}/);
    expect(text.length).toBeGreaterThan(5);
  });
});

describe("skjøten mot opptakssidens `sourceState` — to halvdeler, ett svar", () => {
  // Statuslinjen og TA OPP-siden svarer på det samme spørsmålet fra hver sin
  // kant: `soundChosen` avgjør om skinnen får si «Alt er klart», `sourceState`
  // om Start får trykkes. To sanne halvdeler som er uenige i skjøten er
  // nøyaktig formen `reference-seam-bugs` handler om — og her ville den vært
  // synlig for en frivillig i ett blikk: grønn linje, død knapp.
  const settingsWith = (over: Partial<Settings>): Settings => ({
    ...SETTINGS_DEFAULTS,
    ...over,
  });

  const CASES: Array<[string, Partial<Settings>, DeviceFact[] | null]> = [
    ["ingenting valgt, listen lest", {}, []],
    ["ingenting valgt, listen ikke lest", {}, null],
    ["navn uten id, listen ikke lest", { deviceName: "Behringer X32" }, null],
    [
      "navn uten id, listen lest",
      { deviceName: "Behringer X32" },
      [{ id: "x32", name: "Behringer X32", channels: 32, isDefault: false }],
    ],
    [
      "valgt og til stede",
      { deviceId: "x32", deviceName: "Behringer X32" },
      [{ id: "x32", name: "Behringer X32", channels: 32, isDefault: false }],
    ],
    [
      "valgt, listen ikke lest",
      { deviceId: "x32", deviceName: "Behringer X32" },
      null,
    ],
  ];

  it.each(CASES)(
    "%s — skinnen sier aldri «ready» når Start er sperret",
    (_name, over, devices) => {
      const s = settingsWith(over);
      const source = sourceState(s, devices);
      const line = statusLine({
        isRecording: false,
        soundChosen: soundChosen(s, devices),
        roomMinutes: 600,
        nextAtMs: null,
      });
      if (source.kind === "no-source") {
        expect(line.kind).toBe("nosound");
      } else {
        expect(line.kind).not.toBe("nosound");
      }
    },
  );
});
