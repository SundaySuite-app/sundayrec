/**
 * Tabellen som holder «Start er sperret til en lydkilde er valgt» ærlig.
 *
 * Den ene raden som betyr mest er `deviceId: null` med et `deviceName` som
 * står igjen fra en migrering: den ser besvart ut og er det ikke. Fjern
 * sperren i `sourceState`, og både den raden og `e2e/record.spec.ts` sin
 * mutasjonsprøve blir røde.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";
import { DOT } from "@lib/ui/dot";

import type { Settings } from "../../state/settings";
import {
  basename,
  capitalizeFirst,
  defaultDeviceOf,
  formatBytes,
  formatClock,
  nativeErrorDetail,
  nativeErrorSuffix,
  nativeErrorSuffixFromText,
  qualityReasonSuffix,
  sourceState,
  spanOfMinutes,
  spanOfSeconds,
  type DeviceFact,
  type SourceKind,
} from "./record-core";

function s(over: Partial<Settings> = {}): Settings {
  return { ...SETTINGS_DEFAULTS, ...over };
}

const X32: DeviceFact = {
  id: "x32",
  name: "Behringer X32",
  channels: 32,
  isDefault: false,
};
const BUILT_IN: DeviceFact = {
  id: "builtin",
  name: "MacBook Pro Microphone",
  channels: 2,
  isDefault: true,
};

describe("sourceState", () => {
  const rows: Array<{
    name: string;
    settings: Settings;
    devices: readonly DeviceFact[] | null;
    kind: SourceKind;
    canStart: boolean;
  }> = [
    {
      name: "fersk installasjon — ingenting valgt",
      settings: s({ deviceId: null, deviceName: null }),
      devices: [BUILT_IN],
      kind: "no-source",
      canStart: false,
    },
    {
      name: "et navn uten en id er IKKE et valg",
      settings: s({ deviceId: null, deviceName: "Behringer X32" }),
      devices: [X32],
      kind: "no-source",
      canStart: false,
    },
    {
      name: "tom id teller som ingen id",
      settings: s({ deviceId: "   ", deviceName: "Behringer X32" }),
      devices: [X32],
      kind: "no-source",
      canStart: false,
    },
    {
      name: "valgt og til stede",
      settings: s({ deviceId: "x32", deviceName: "Behringer X32" }),
      devices: [X32, BUILT_IN],
      kind: "ready",
      canStart: true,
    },
    {
      name: "valgt, men mikseren er borte",
      settings: s({ deviceId: "x32", deviceName: "Behringer X32" }),
      devices: [BUILT_IN],
      kind: "source-missing",
      canStart: true,
    },
    {
      name: "listen er ikke lest ennå — ingen påstand om at den mangler",
      settings: s({ deviceId: "x32", deviceName: "Behringer X32" }),
      devices: null,
      kind: "ready",
      canStart: true,
    },
    {
      name: "maskinens egen mikrofon er et ekte valg når den ER valgt",
      settings: s({ deviceId: "builtin", deviceName: "Innebygd mikrofon" }),
      devices: [BUILT_IN],
      kind: "ready",
      canStart: true,
    },
    {
      name: "tom enhetsliste er et ekte svar: valget finnes ikke",
      settings: s({ deviceId: "x32", deviceName: "Behringer X32" }),
      devices: [],
      kind: "source-missing",
      canStart: true,
    },
  ];

  for (const row of rows) {
    it(row.name, () => {
      const state = sourceState(row.settings, row.devices);
      expect(state.kind).toBe(row.kind);
      expect(state.canStart).toBe(row.canStart);
    });
  }

  it("navnet kommer fra enheten når den finnes, og fra basen når den ikke gjør det", () => {
    expect(
      sourceState(s({ deviceId: "x32", deviceName: "Gammelt navn" }), [X32])
        .name,
    ).toBe("Behringer X32");
    expect(
      sourceState(s({ deviceId: "x32", deviceName: "Gammelt navn" }), []).name,
    ).toBe("Gammelt navn");
  });

  it("kanalparet vises bare for enheter med flere enn to kanaler", () => {
    const withPair = s({
      deviceId: "x32",
      deviceName: "Behringer X32",
      deviceChannels: { x32: { channelL: 14, channelR: 15 } },
    });
    expect(sourceState(withPair, [X32]).pair).toEqual({ l: 15, r: 16 });

    const stereo = s({
      deviceId: "builtin",
      deviceName: "Innebygd",
      deviceChannels: { builtin: { channelL: 0, channelR: 1 } },
    });
    expect(sourceState(stereo, [BUILT_IN]).pair).toBeNull();
  });
});

describe("defaultDeviceOf", () => {
  it("finner vertens standardenhet", () => {
    expect(defaultDeviceOf([X32, BUILT_IN])?.id).toBe("builtin");
  });
  it("svarer null når ingen er standard — da finnes det ingen nødutgang", () => {
    expect(defaultDeviceOf([X32])).toBeNull();
    expect(defaultDeviceOf([])).toBeNull();
    expect(defaultDeviceOf(null)).toBeNull();
  });
});

describe("formatClock", () => {
  const rows: Array<[number, string]> = [
    [0, "0:00:00"],
    [1_000, "0:00:01"],
    [61_000, "0:01:01"],
    [2537 * 1000, "0:42:17"],
    [3_600_000, "1:00:00"],
    [3_734_000, "1:02:14"],
    [36_000_000, "10:00:00"],
    [-5_000, "0:00:00"],
    [Number.NaN, "0:00:00"],
  ];
  for (const [ms, want] of rows) {
    it(`${ms} → ${want}`, () => expect(formatClock(ms)).toBe(want));
  }
});

describe("spanOfMinutes", () => {
  it("hele timer", () => {
    expect(spanOfMinutes(840)).toEqual({
      kind: "hours",
      hours: 14,
      minutes: "0",
    });
  });
  it("timer og minutter, med nullpolstring", () => {
    expect(spanOfMinutes(62)).toEqual({
      kind: "hoursMinutes",
      hours: 1,
      minutes: "02",
    });
  });
  it("under en time", () => {
    expect(spanOfMinutes(45)).toEqual({
      kind: "minutes",
      hours: 0,
      minutes: "45",
    });
  });
  it("null minutter er «45 min»-formens null, ikke «ingenting»", () => {
    expect(spanOfMinutes(0)).toEqual({
      kind: "minutes",
      hours: 0,
      minutes: "0",
    });
  });
  it("ukjent er ingen påstand", () => {
    expect(spanOfMinutes(null).kind).toBe("none");
    expect(spanOfMinutes(Number.NaN).kind).toBe("none");
    expect(spanOfMinutes(-1).kind).toBe("none");
  });
});

describe("spanOfSeconds", () => {
  it("3734 s → 1 t 02 min", () => {
    expect(spanOfSeconds(3734)).toEqual({
      kind: "hoursMinutes",
      hours: 1,
      minutes: "02",
    });
  });
  it("ukjent varighet sier ingenting", () => {
    expect(spanOfSeconds(null).kind).toBe("none");
  });
});

describe("formatBytes", () => {
  it("megabyte under tusen", () => {
    expect(formatBytes(112_000_000, "en")).toBe("112 MB");
  });
  it("gigabyte over", () => {
    expect(formatBytes(1_400_000_000, "en")).toBe("1.4 GB");
  });
  it("ukjent størrelse er tom, ikke «0 MB»", () => {
    expect(formatBytes(null, "en")).toBe("");
    expect(formatBytes(-1, "en")).toBe("");
  });
});

describe("basename", () => {
  it("tar filnavnet ut av en sti", () => {
    expect(basename("/Users/a/SundayRec/2026-08-23.mp3")).toBe(
      "2026-08-23.mp3",
    );
    expect(basename("C:\\Opptak\\gudstjeneste.wav")).toBe("gudstjeneste.wav");
    expect(basename("bare-navnet.mp3")).toBe("bare-navnet.mp3");
  });
});

describe("capitalizeFirst", () => {
  it("løfter første tegn, og lar resten stå", () => {
    expect(capitalizeFirst("søndag 16. august", "no")).toBe(
      "Søndag 16. august",
    );
    expect(capitalizeFirst("Sunday 16 August", "en")).toBe("Sunday 16 August");
  });
  it("tom tekst er tom tekst", () => {
    expect(capitalizeFirst("", "no")).toBe("");
  });
});

describe("nativeErrorSuffix", () => {
  it("kjente koder peker på katalogens egne nøkler", () => {
    expect(nativeErrorSuffix("no_save_folder")).toBe("errorNoSaveFolder");
    expect(nativeErrorSuffix("device_not_found")).toBe("errorDeviceNotFound");
    expect(nativeErrorSuffix("no_device")).toBe("errorDeviceNotFound");
  });
  it("ukjent kode blir «ukjent», ikke rå maskintekst", () => {
    expect(nativeErrorSuffix("something_new")).toBe("errorUnknown");
    expect(nativeErrorSuffix(null)).toBe("errorUnknown");
  });
  it("finner koden inne i en lengre feiltekst", () => {
    expect(nativeErrorSuffixFromText("no_save_folder")).toBe(
      "errorNoSaveFolder",
    );
    expect(nativeErrorSuffixFromText("recorder: disk_full at 12:00")).toBe(
      "errorDiskFull",
    );
    expect(nativeErrorSuffixFromText("")).toBe("errorUnknown");
  });

  it("F1-M1: de fire kodene som falt på «ukjent» har hver sin setning", () => {
    // Alle fire kom fra ekte søndagsfeil og alle fire ble til «Noe gikk galt
    // under opptak»: startvakten som ga opp, ffmpeg som døde midt i,
    // kameraet som aldri åpnet, og sammenslåingen som feilet.
    // `scripts/check-error-codes.mjs` er skrallen som holder dem her — dette
    // er raden som sier hvilken setning hver av dem peker på.
    expect(nativeErrorSuffix("start_timeout")).toBe("errorStartTimeout");
    expect(nativeErrorSuffix("ffmpeg_exited")).toBe("errorEngineExited");
    expect(nativeErrorSuffix("video_capture_failed")).toBe("errorVideoCapture");
    expect(nativeErrorSuffix("mux_failed")).toBe("errorMux");
  });

  it("hver kode i tabellen peker på en nøkkel som FINNES i både no og en", () => {
    // Samme forbehold som `qualityReasonSuffix` under: tabellen er navnene,
    // katalogene er sannheten. En rad som pekte på en nøkkel ingen hadde
    // skrevet ville gitt et TOMT banner der det sto en feilmelding — og et
    // tomt banner er nøyaktig den stillheten gaten finnes for å avslutte.
    // Kodene leses ut av KILDEN og ikke skrevet av: en rad som legges til i
    // `record-core.ts` uten oversettelse felles her, ikke av en oppdaget
    // søndag. (Samme metode som `check-error-codes.mjs` bruker på den andre
    // siden av skjøten — den spør om Rust og tabellen er enige, denne om
    // tabellen og katalogene er det.)
    const src = readFileSync(
      join(import.meta.dirname, "./record-core.ts"),
      "utf8",
    );
    const table = src.slice(
      src.indexOf("const NATIVE_ERRORS"),
      src.indexOf("export function nativeErrorSuffix"),
    );
    const codes = [...table.matchAll(/\n\s{2}(\w+):\s*"/g)].map((m) => m[1]);
    expect(codes.length).toBeGreaterThan(15);
    for (const lang of ["no", "en"]) {
      const cat = JSON.parse(
        readFileSync(
          join(import.meta.dirname, `../../../legacy/locales/${lang}.json`),
          "utf8",
        ),
      ) as { recording: Record<string, string> };
      for (const code of codes) {
        const suffix = nativeErrorSuffix(code);
        expect(suffix, `${code} mangler i tabellen`).not.toBe("errorUnknown");
        expect(
          typeof cat.recording[suffix] === "string" &&
            cat.recording[suffix].length > 0,
          `${lang}.json mangler recording.${suffix} (kode ${code})`,
        ).toBe(true);
      }
    }
  });
});

describe("nativeErrorDetail", () => {
  it("kjent kode: bare katalogteksten — motorens linje er diagnostikk", () => {
    expect(
      nativeErrorDetail("Disken er full", "disk_full", "no space left"),
    ).toBe("Disken er full");
  });

  it("ukjent kode: motorens egen setning føyes til", () => {
    // Regelen `state/backend-warning.ts` bruker for `msg`. Uten den er
    // «errorUnknown» alt som står igjen — sant, og uten et eneste faktum.
    expect(
      nativeErrorDetail("Noe gikk galt", "cosmic_rays", "avfoundation: -50"),
    ).toBe(`Noe gikk galt${DOT}avfoundation: -50`);
  });

  it("ingen kode i det hele tatt: fortsatt motorens setning", () => {
    expect(nativeErrorDetail("Noe gikk galt", null, "ffmpeg died")).toBe(
      `Noe gikk galt${DOT}ffmpeg died`,
    );
  });

  it("tom eller manglende melding gir ingen hengende separator", () => {
    expect(nativeErrorDetail("Noe gikk galt", "cosmic_rays", null)).toBe(
      "Noe gikk galt",
    );
    expect(nativeErrorDetail("Noe gikk galt", "cosmic_rays", "   ")).toBe(
      "Noe gikk galt",
    );
  });
});

describe("qualityReasonSuffix", () => {
  it("hver kode peker på en nøkkel som FINNES i både no og en", () => {
    // Katalogene er sannheten her; tabellen er bare navnene. En rad som pekte
    // på en nøkkel ingen hadde skrevet ville gitt tom tekst i banneret som
    // sier «ikke stol på dette opptaket».
    // Kodene leses ut av den GENERERTE bindingen, ikke skrevet av: den er
    // Rustens `QualityReason`, og en variant som skifter stavemåte skal gjøre
    // denne testen rød i stedet for å bli et oppslag som bommer i stillhet.
    const binding = readFileSync(
      join(import.meta.dirname, "../../../legacy/bindings/QualityReason.ts"),
      "utf8",
    );
    const codes = [...binding.matchAll(/"([a-z-]+)"/g)].map((m) => m[1]);
    expect(codes.length).toBe(9);
    for (const lang of ["no", "en"]) {
      const cat = JSON.parse(
        readFileSync(
          join(import.meta.dirname, `../../../legacy/locales/${lang}.json`),
          "utf8",
        ),
      ) as { recording: Record<string, string> };
      for (const code of codes) {
        const suffix = qualityReasonSuffix(code);
        expect(suffix, code).not.toBeNull();
        expect(
          typeof cat.recording[suffix as string] === "string" &&
            cat.recording[suffix as string].length > 0,
          `${lang}.json mangler recording.${suffix}`,
        ).toBe(true);
      }
    }
  });

  it("en UKJENT kode gir `null`, ikke en generisk setning", () => {
    // `null` er hele kontrakten mot siden: da vises motorens EGEN prosalinje
    // på samme indeks. Å bytte en sann setning mot en generisk «ukjent årsak»
    // ville vært å handle informasjon for språk.
    expect(qualityReasonSuffix("cosmic_rays")).toBeNull();
  });
});
