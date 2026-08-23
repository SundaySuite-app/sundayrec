/**
 * Tabellen eieren skal kunne overprøve — festet, tall for tall.
 *
 * Dette er ikke en test av at koden gjør det den gjør. Det er en test av at de
 * to preset-id-ene FINNES i kjernens egen liste, og at ingen kan bytte dem uten
 * at noe sier fra: `speech-clear` og `music-speech` er strenger som krysser
 * IPC og slås opp med `get_preset_by_id` på den andre siden, der en tastefeil
 * blir `unknown_preset` og en mislykket eksport — ikke en typefeil.
 */

import { describe, expect, it } from "vitest";
import { masterPresetIds } from "./test-support/rust-presets";

import {
  channelCodeKey,
  DEFAULT_SOUND_PROFILE,
  listenStartSec,
  LISTEN_SPAN_SEC,
  MIXED_MASTER_PRESET,
  soundExportFields,
  SOUND_PROFILES,
  specFor,
  SPEECH_MASTER_PRESET,
} from "./sound-profiles";

const REPAIR = { mode: "gainDb", leftDb: 0, rightDb: -2.4 };

describe("lydprofilene", () => {
  it("de to preset-id-ene finnes i kjernens egen liste", () => {
    // Leses ut av `crates/sundayrec-core/src/mastering.rs`. Bytter noen navnet
    // der, eller her, går denne rød — i stedet for at eksporten svarer
    // «unknown_preset» først på søndag.
    const ids = masterPresetIds();
    expect(ids).toContain(SPEECH_MASTER_PRESET);
    expect(ids).toContain(MIXED_MASTER_PRESET);
  });

  it("«Tale» er standarden, og den er anbefalt", () => {
    expect(DEFAULT_SOUND_PROFILE).toBe("speech");
    expect(SOUND_PROFILES[0]).toBe("speech");
  });

  it("de tre navnene peker på nøyaktig disse verdiene", () => {
    expect(specFor("speech")).toEqual({
      masterPreset: "speech-clear",
      channelRepair: true,
    });
    expect(specFor("mixed")).toEqual({
      masterPreset: "music-speech",
      channelRepair: true,
    });
    expect(specFor("none")).toEqual({
      masterPreset: undefined,
      channelRepair: false,
    });
  });
});

describe("nyttelastens lydfelter", () => {
  it("«Tale» sender mastring-presettet og reparasjonen — og ingen stemmekjede", () => {
    expect(
      soundExportFields({
        profile: "speech",
        useMixer: false,
        repair: REPAIR,
      }),
    ).toEqual({ masterPreset: "speech-clear", channelRepair: REPAIR });
  });

  it("«Tale og musikk» bytter bare presettet", () => {
    expect(
      soundExportFields({ profile: "mixed", useMixer: false, repair: null }),
    ).toEqual({ masterPreset: "music-speech", channelRepair: undefined });
  });

  it("«Ingen» sender ingenting — heller ikke reparasjonen", () => {
    expect(
      soundExportFields({ profile: "none", useMixer: true, repair: REPAIR }),
    ).toEqual({});
  });

  it("mikseren ERSTATTER profilen: `processing` inn, `masterPreset` ut", () => {
    // Stablingen bakenden advarer mot i `auto_process`: to høypass, to
    // kompressorer, to EQ-kurver. Går denne rød fordi `masterPreset` er tilbake
    // i objektet, er det nøyaktig den feilen som er tilbake.
    const out = soundExportFields({
      profile: "speech",
      useMixer: true,
      processing: { highpassEnabled: true },
      repair: REPAIR,
    });
    expect(out.processing).toEqual({ highpassEnabled: true });
    expect(out.masterPreset).toBeUndefined();
    expect(out.channelRepair).toBe(REPAIR);
  });

  it("ingen vei setter `vocalChainPreset`", () => {
    for (const profile of SOUND_PROFILES) {
      for (const useMixer of [true, false]) {
        const out = soundExportFields({
          profile,
          useMixer,
          processing: {},
          repair: REPAIR,
        });
        expect(out.vocalChainPreset).toBeUndefined();
      }
    }
  });
});

describe("lytteutsnittet", () => {
  it("ligger midt i prekenvinduet", () => {
    // 210–420 → midten er 315, og 20 sekunder derfra begynner på 305.
    expect(listenStartSec({ start: 210, end: 420 }, 600)).toBe(305);
  });

  it("uten et vindu brukes midten av fila", () => {
    expect(listenStartSec(null, 600)).toBe(290);
  });

  it("klemmes så hele utsnittet får plass", () => {
    // Et vindu helt i halen: startpunktet trekkes inn så det blir 20 sekunder
    // å høre på, ikke fem.
    expect(listenStartSec({ start: 590, end: 600 }, 600)).toBe(
      600 - LISTEN_SPAN_SEC,
    );
    // Og en fil som er kortere enn utsnittet begynner på null.
    expect(listenStartSec({ start: 0, end: 8 }, 8)).toBe(0);
  });

  it("en varighet vi ikke kjenner gir null, ikke NaN", () => {
    expect(listenStartSec(null, 0)).toBe(0);
    expect(listenStartSec(null, Number.NaN)).toBe(0);
  });
});

describe("kanaldiagnosen", () => {
  it("de seks kodene har hver sin legacy-nøkkel", () => {
    expect(channelCodeKey("dead_left")).toBe("chanDeadLeft");
    expect(channelCodeKey("mono")).toBe("chanMono");
    expect(channelCodeKey("balanced")).toBe("chanBalanced");
  });

  it("en ukjent kode gir ingenting — ikke en råkode på skjermen", () => {
    expect(channelCodeKey("noe_nytt")).toBeNull();
    expect(channelCodeKey(undefined)).toBeNull();
  });
});
