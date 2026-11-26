import { describe, expect, it } from "vitest";

import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";

import type { Settings } from "../../state/settings";
import {
  autoValue,
  cameraExpandable,
  cameraValue,
  type CameraFacts,
} from "./card-values-core";

function settings(over: Partial<Settings> = {}): Settings {
  return { ...(SETTINGS_DEFAULTS as Settings), ...over };
}

function camera(over: Partial<CameraFacts> = {}): CameraFacts {
  return { enabled: false, chosen: "", count: null, failed: false, ...over };
}

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
});
