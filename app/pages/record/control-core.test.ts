import { describe, expect, it } from "vitest";

import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";

import type { Settings } from "../../state/settings";
import { decisionsFor, type DecisionFacts } from "../setup/decisions-core";
import {
  autoExpandable,
  autoValue,
  cameraExpandable,
  cameraValue,
  CONTROL_IDS,
  decisionRows,
  isControlId,
  STACK_IDS,
  toneOf,
  type CameraFacts,
} from "./control-core";

function settings(over: Partial<Settings> = {}): Settings {
  return { ...(SETTINGS_DEFAULTS as Settings), ...over };
}

function facts(over: Partial<DecisionFacts> = {}): DecisionFacts {
  return {
    settings: settings(),
    devices: [],
    diskFreeBytes: null,
    roomMinutes: null,
    emailTransport: null,
    locale: "no",
    vuWord: null,
    ...over,
  };
}

function camera(over: Partial<CameraFacts> = {}): CameraFacts {
  return { enabled: false, chosen: "", count: null, failed: false, ...over };
}

describe("kortlista", () => {
  it("er de seks ankrene ruteren har lov til å love", () => {
    expect([...CONTROL_IDS]).toEqual([
      "sound",
      "folder",
      "quality",
      "camera",
      "auto",
      "notify",
    ]);
    for (const id of CONTROL_IDS) expect(isControlId(id)).toBe(true);
  });

  it("sier nei til alt annet — også til tomt og fraværende", () => {
    // Vakten mot at et vilkårlig `?goto=`-suffiks folder ut «et kort» som ikke
    // finnes: et anker som ikke er en kort-id skal bare rulle, ikke åpne noe.
    for (const wrong of ["", "advanced", "engine", "church", "SOUND"]) {
      expect(isControlId(wrong)).toBe(false);
    }
    expect(isControlId(null)).toBe(false);
    expect(isControlId(undefined)).toBe(false);
  });

  it("legger kilden i venstrekolonnen og de fem andre i stabelen", () => {
    // `sound` er det LEVENDE kortet — kilde, måler og Start hører sammen — så
    // det er det ene som ikke står i stabelen til høyre.
    expect([...STACK_IDS]).toEqual([
      "folder",
      "quality",
      "camera",
      "auto",
      "notify",
    ]);
    expect(STACK_IDS).not.toContain("sound");
  });
});

describe("tonen på et kort", () => {
  it("er gul BARE når spørsmålet ikke er besvart", () => {
    const [, folder] = decisionsFor(facts());
    // Ingen mappe ⇒ todo ⇒ gul.
    expect(folder.status).toBe("todo");
    expect(toneOf(folder)).toBe("warn");
  });

  it("er nøytral når disken ikke har svart ennå — `unknown` er ikke et halvt nei", () => {
    // Et gult kort som blir nøytralt etter 100 ms er nøyaktig det som lærer
    // folk å ignorere gult.
    const [, folder] = decisionsFor(
      facts({
        settings: settings({ saveFolder: "/Users/x/SundayRec" }),
        diskFreeBytes: null,
      }),
    );
    expect(folder.status).toBe("unknown");
    expect(toneOf(folder)).toBe("neutral");
  });
});

describe("stabelens beslutningsrader", () => {
  it("er mappe, kvalitet og varsling — i den rekkefølgen, med svaret som data", () => {
    const rows = decisionRows(
      decisionsFor(
        facts({
          settings: settings({ saveFolder: "/Users/x/SundayRec" }),
          diskFreeBytes: 250_000_000_000,
        }),
      ),
    );
    expect(rows.map((r) => r.id)).toEqual(["folder", "quality", "notify"]);
    expect(rows[0].answer).toEqual({
      key: "path",
      path: "/Users/x/SundayRec",
    });
    expect(rows[0].tone).toBe("neutral");
    expect(rows[0].needsSetUp).toBe(false);
    // Detaljen følger med: den gule raden skal si hva den koster, ikke bare at
    // noe mangler.
    expect(rows[0].detail).toEqual({
      key: "space",
      freeBytes: 250_000_000_000,
      roomMinutes: null,
    });
  });

  it("sier «Sett opp» bare når det bokstavelig talt ikke står et svar", () => {
    const rows = decisionRows(decisionsFor(facts()));
    const folder = rows.find((r) => r.id === "folder")!;
    const notify = rows.find((r) => r.id === "notify")!;
    // Ingen mappe, og ingen som får beskjed: begge er ubesvart.
    expect(folder.needsSetUp).toBe(true);
    expect(notify.needsSetUp).toBe(true);
    // Kvaliteten har alltid et svar — standarden ER et svar.
    expect(rows.find((r) => r.id === "quality")!.needsSetUp).toBe(false);
  });

  it("lar en mappe som er valgt, men umålt, være noe man ENDRER", () => {
    const rows = decisionRows(
      decisionsFor(
        facts({ settings: settings({ saveFolder: "/Users/x/SundayRec" }) }),
      ),
    );
    expect(rows[0].needsSetUp).toBe(false);
  });
});

describe("kamera-kortets kompaktverdi", () => {
  it.each([
    [
      "av: kortet sier hva tillegget er, ikke hvilket kamera som ville blitt brukt",
      camera({ enabled: false, chosen: "Logitech BRIO" }),
      { key: "off" },
    ],
    [
      "en FEILET lesning er ikke «ingen kameraer»",
      camera({ enabled: true, failed: true, count: 0 }),
      { key: "listError" },
    ],
    [
      "en ekte tom liste sier at det ikke er noe kamera",
      camera({ enabled: true, count: 0 }),
      { key: "none" },
    ],
    [
      "ikke lest ennå påstår ingenting om antallet",
      camera({ enabled: true, count: null }),
      { key: "noneChosen" },
    ],
    [
      "valgt kamera er svaret",
      camera({ enabled: true, count: 1, chosen: "Logitech BRIO" }),
      { key: "name", name: "Logitech BRIO" },
    ],
  ])("%s", (_name, given, expected) => {
    expect(cameraValue(given)).toEqual(expected);
  });

  it("kan bare foldes ut når tillegget er PÅ", () => {
    // Kroppen ER kameravalget. Et valg mellom enheter som ikke skal brukes er
    // en kontroll uten virkning; bryteren i topplinja er affordansen da.
    expect(cameraExpandable(camera({ enabled: false }))).toBe(false);
    expect(cameraExpandable(camera({ enabled: true }))).toBe(true);
  });
});

describe("auto-kortets kompaktverdi", () => {
  const slot = { days: [6], start: "11:00", stop: "12:30", max: null };

  it("er tiden som gjelder når flagget OG en tid står", () => {
    expect(
      autoValue(settings({ autoRecordEnabled: true, slots: [slot] })),
    ).toEqual({ key: "plan", day: 6, start: "11:00", minutes: 90 });
  });

  it("er «av» når flagget er av — også med tidene i behold", () => {
    // Av sletter ingenting: tidene blir stående i basen og kommer tilbake
    // nøyaktig som de sto.
    expect(
      autoValue(settings({ autoRecordEnabled: false, slots: [slot] })),
    ).toEqual({ key: "off" });
  });

  it("er «av» når flagget står, men ingen tid finnes", () => {
    // Et armert flagg uten en eneste slot lover et opptak ingen har bedt om.
    expect(autoValue(settings({ autoRecordEnabled: true, slots: [] }))).toEqual(
      { key: "off" },
    );
  });

  it("kan bare foldes ut når det finnes en tid å redigere", () => {
    expect(autoExpandable(settings({ slots: [] }))).toBe(false);
    expect(
      autoExpandable(settings({ autoRecordEnabled: true, slots: [slot] })),
    ).toBe(true);
  });
});
