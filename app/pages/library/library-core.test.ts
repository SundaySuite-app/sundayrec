import { describe, expect, it } from "vitest";

import type { RecordingEntry } from "@legacy/types";
import { spanOfSeconds } from "../record/record-core";
import {
  autoDeleteLine,
  dueLine,
  durationOf,
  filterEntries,
  isVideoPath,
  matchesQuery,
  MIN_QUERY_LENGTH,
  rowSpan,
  sortNewestFirst,
  startedAtOf,
  toLibraryRows,
  totalSeconds,
} from "./library-core";

/** En oppføring slik api-shimmens `rowToEntry` faktisk leverer den. */
function entry(over: Partial<RecordingEntry> = {}): RecordingEntry {
  const timestamp = over.timestamp ?? 1_754_400_000_000;
  return {
    timestamp,
    startedAt: timestamp,
    date: Number.isFinite(timestamp) ? new Date(timestamp).toISOString() : "",
    startTime: "",
    path: "/Opptak/2026-08-05 Gudstjeneste.mp3",
    filename: "2026-08-05 Gudstjeneste.mp3",
    duration: "1t 00m",
    status: "ok",
    durationSec: 3600,
    ...over,
  };
}

describe("startedAtOf", () => {
  it("bruker startedAt — når økta BEGYNTE", () => {
    const row = entry({ timestamp: 1_000_000, startedAt: 990_000 });
    expect(startedAtOf(row)).toBe(990_000);
  });

  it("faller tilbake på timestamp når startedAt mangler", () => {
    // En rad uten dato i det hele tatt er verre enn en dato som er litt sen.
    const row = entry({ timestamp: 1_000_000 });
    delete row.startedAt;
    expect(startedAtOf(row)).toBe(1_000_000);
  });

  it("regner 0 og søppel som ukjent", () => {
    expect(startedAtOf(entry({ timestamp: 0, startedAt: 0 }))).toBeNull();
    expect(startedAtOf(entry({ timestamp: NaN, startedAt: NaN }))).toBeNull();
  });
});

describe("durationOf", () => {
  it("regner 0 som UKJENT, ikke som null sekunder", () => {
    // `rowToEntry` gjør en manglende `duration_ms` til 0, så 0 er tvetydig.
    // «0 min» er en påstand vi ikke kan stå for — WKWebView-proben i P2 fant
    // nøyaktig den setningen på eierens egen maskin.
    expect(durationOf(entry({ durationSec: 0 }))).toBeNull();
    expect(durationOf(entry({ durationSec: 3600 }))).toBe(3600);
  });
});

describe("rowSpan", () => {
  it("regner 0 som ukjent", () => {
    expect(rowSpan(null)).toEqual({ kind: "unknown", seconds: 0 });
    expect(rowSpan(0)).toEqual({ kind: "unknown", seconds: 0 });
    expect(rowSpan(-5)).toEqual({ kind: "unknown", seconds: 0 });
  });

  it("sier «under ett minutt» der `spanOfSeconds` ville sagt «0 min»", () => {
    // WKWebView-probens funn på eierens egen maskin: fem testopptak fra
    // Qu-5-runden var kortere enn et halvt minutt, og «0 min» er kjent OG
    // usant. Grensen er nøyaktig den `spanOfSeconds` runder på.
    expect(rowSpan(1).kind).toBe("under");
    expect(rowSpan(20).kind).toBe("under");
    expect(rowSpan(29).kind).toBe("under");
    expect(spanOfSeconds(29)).toEqual({
      kind: "minutes",
      hours: 0,
      minutes: "0",
    });
  });

  it("teller fra der avrundingen gir minst ett minutt", () => {
    expect(rowSpan(30)).toEqual({ kind: "span", seconds: 30 });
    expect(rowSpan(3600)).toEqual({ kind: "span", seconds: 3600 });
  });
});

describe("isVideoPath", () => {
  it("avgjør på etternavnet", () => {
    expect(isVideoPath("/x/a.mp4")).toBe(true);
    expect(isVideoPath("/x/a.MOV")).toBe(true);
    expect(isVideoPath("/x/a.mp3")).toBe(false);
    expect(isVideoPath("/x/a")).toBe(false);
    expect(isVideoPath(null)).toBe(false);
  });

  it("lar seg IKKE lure av et notat som heter «Video»", () => {
    // `isVideoRow` i history-core har `note === 'Video'` som reservevei for
    // Electron-importerte rader. Notatet er synlig tekst på raden nå, så den
    // veien ville gitt en Video-brikke til et lydopptak noen skrev «Video» på.
    expect(isVideoPath("/x/a.mp3")).toBe(false);
  });
});

describe("toLibraryRows", () => {
  it("folder lyd + video fra samme økt til ÉN rad med Video-brikke", () => {
    const rows = toLibraryRows([
      entry({
        path: "/Opptak/gudstjeneste.mp4",
        filename: "gudstjeneste.mp4",
        timestamp: 5_000,
        startedAt: 5_000,
      }),
      entry({
        path: "/Opptak/gudstjeneste.wav",
        filename: "gudstjeneste.wav",
        timestamp: 5_000,
        startedAt: 5_000,
      }),
    ]);
    expect(rows).toHaveLength(1);
    expect(rows[0].hasVideo).toBe(true);
    // Lyd-halvdelen er radens eget opptak; videoen henger ved.
    expect(rows[0].filename).toBe("gudstjeneste.wav");
    expect(rows[0].video?.filename).toBe("gudstjeneste.mp4");
  });

  it("gir en video UTEN separat lydfil sin egen rad, også med brikke", () => {
    const rows = toLibraryRows([
      entry({ path: "/Opptak/konsert.mp4", filename: "konsert.mp4" }),
    ]);
    expect(rows).toHaveLength(1);
    expect(rows[0].hasVideo).toBe(true);
    expect(rows[0].video).toBeNull();
  });

  it("legger den nyeste øverst, målt på startedAt", () => {
    const rows = toLibraryRows([
      entry({ path: "/a.mp3", filename: "a", startedAt: 1_000 }),
      entry({ path: "/c.mp3", filename: "c", startedAt: 3_000 }),
      entry({ path: "/b.mp3", filename: "b", startedAt: 2_000 }),
    ]);
    expect(rows.map((r) => r.filename)).toEqual(["c", "b", "a"]);
  });

  it("sorterer på startedAt og ikke på timestamp", () => {
    // Den ene raden varte to timer, den andre ti minutter. Sortert på
    // `timestamp` (når raden ble SKREVET) ville den korte lagt seg først.
    const long = entry({
      path: "/lang.mp3",
      filename: "lang",
      startedAt: 1_000,
      timestamp: 8_200_000,
    });
    const short = entry({
      path: "/kort.mp3",
      filename: "kort",
      startedAt: 2_000,
      timestamp: 602_000,
    });
    expect(toLibraryRows([short, long]).map((r) => r.filename)).toEqual([
      "kort",
      "lang",
    ]);
  });

  it("lar rader uten dato ligge sist", () => {
    const rows = toLibraryRows([
      entry({
        path: "/ukjent.mp3",
        filename: "ukjent",
        startedAt: 0,
        timestamp: 0,
      }),
      entry({ path: "/kjent.mp3", filename: "kjent", startedAt: 9_000 }),
    ]);
    expect(rows.map((r) => r.filename)).toEqual(["kjent", "ukjent"]);
    expect(rows[1].atMs).toBeNull();
  });

  it("tar med notatet, og bare når det står noe der", () => {
    const rows = toLibraryRows([
      entry({ path: "/a.mp3", note: "  Dåp  " }),
      entry({ path: "/b.mp3", note: "   " }),
    ]);
    const byPath = new Map(rows.map((r) => [r.path, r.note]));
    expect(byPath.get("/a.mp3")).toBe("Dåp");
    expect(byPath.get("/b.mp3")).toBeNull();
  });

  it("gir stiløse rader hver sin nøkkel", () => {
    const rows = toLibraryRows([
      entry({ path: undefined, filename: "x" }),
      entry({ path: undefined, filename: "y" }),
    ]);
    expect(rows).toHaveLength(2);
    expect(new Set(rows.map((r) => r.key)).size).toBe(2);
    expect(rows.every((r) => r.path === null)).toBe(true);
  });
});

describe("sortNewestFirst", () => {
  it("er stabil for like tidspunkter og muterer ikke innputen", () => {
    const input = [
      entry({ filename: "a", startedAt: 1_000 }),
      entry({ filename: "b", startedAt: 1_000 }),
    ];
    const copy = input.slice();
    expect(sortNewestFirst(input).map((e) => e.filename)).toEqual(["a", "b"]);
    expect(input).toEqual(copy);
  });
});

describe("matchesQuery / filterEntries", () => {
  const rows = [
    entry({
      path: "/a.mp3",
      filename: "2026-08-09 Bønnemøte.mp3",
      note: "Dåp",
    }),
    entry({ path: "/b.mp3", filename: "2026-08-02 Gudstjeneste.mp3" }),
  ];

  it("treffer filnavn uten hensyn til store bokstaver", () => {
    expect(matchesQuery(rows[0], "bønnemøte")).toBe(true);
    expect(matchesQuery(rows[0], "BØNNEMØTE")).toBe(true);
    expect(matchesQuery(rows[1], "bønnemøte")).toBe(false);
  });

  it("treffer notatet", () => {
    expect(matchesQuery(rows[0], "dåp")).toBe(true);
  });

  it("treffer datoen ORDRETT — den er en ISO-streng", () => {
    const iso = rows[0].date.slice(0, 10);
    expect(matchesQuery(rows[0], iso)).toBe(true);
  });

  it("filtrerer ingenting under to tegn", () => {
    expect(MIN_QUERY_LENGTH).toBe(2);
    expect(filterEntries(rows, "b")).toHaveLength(2);
    expect(filterEntries(rows, "  ")).toHaveLength(2);
    expect(filterEntries(rows, "bø")).toHaveLength(1);
  });
});

describe("totalSeconds", () => {
  it("teller en økt med kamera ÉN gang, ikke to", () => {
    // history-cores `historyTotals` summerer per OPPFØRING, så et opptak med
    // kamera — to rader i basen — bidrar med sin egen lengde to ganger.
    const rows = toLibraryRows([
      entry({ path: "/g.mp4", filename: "g.mp4", durationSec: 3600 }),
      entry({ path: "/g.wav", filename: "g.wav", durationSec: 3600 }),
    ]);
    expect(totalSeconds(rows)).toBe(3600);
  });

  it("lar en ukjent varighet bidra med ingenting", () => {
    const rows = toLibraryRows([
      entry({ path: "/a.mp3", durationSec: 0 }),
      entry({ path: "/b.mp3", durationSec: 600 }),
    ]);
    expect(totalSeconds(rows)).toBe(600);
  });
});

describe("autoDeleteLine", () => {
  it("0, negativt og ugyldig er «av»", () => {
    for (const value of [0, -1, null, undefined, NaN]) {
      expect(autoDeleteLine(value).kind).toBe("off");
    }
  });

  it("1 har sin egen form — «1 dager» finnes ikke", () => {
    expect(autoDeleteLine(1)).toEqual({ kind: "oneDay", days: 1 });
  });

  it("resten teller", () => {
    expect(autoDeleteLine(90)).toEqual({ kind: "days", days: 90 });
    expect(autoDeleteLine(2.7)).toEqual({ kind: "days", days: 2 });
  });
});

describe("dueLine", () => {
  it("0 er neste opprydding, ikke «i dag»", () => {
    // Sveipen går hver 12. time, ikke ved midnatt — «i dag» ville vært en
    // påstand om et tidspunkt ingen kjenner.
    expect(dueLine(0)).toEqual({ kind: "now", days: 0 });
    expect(dueLine(-3)).toEqual({ kind: "now", days: 0 });
  });

  it("1 er i morgen", () => {
    expect(dueLine(1)).toEqual({ kind: "tomorrow", days: 1 });
  });

  it("resten teller", () => {
    expect(dueLine(27)).toEqual({ kind: "days", days: 27 });
  });
});
