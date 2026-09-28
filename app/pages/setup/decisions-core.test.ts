import { describe, expect, it } from "vitest";
import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";

import {
  answeredCount,
  channelPairFor,
  decideChurch,
  decideFolder,
  decideNotify,
  decideQuality,
  decideSound,
  channelPairs,
  decisionsFor,
  needsSetUp,
  qualityIdFor,
  type DecisionFacts,
  type DecisionStatus,
} from "./decisions-core";
import type { Settings } from "../../state/settings";

/** Fabrikkfersk profil + det raden faktisk handler om. */
function facts(over: Partial<DecisionFacts> = {}): DecisionFacts {
  return {
    settings: { ...SETTINGS_DEFAULTS },
    devices: null,
    diskFreeBytes: null,
    roomMinutes: null,
    locale: "no",
    vuWord: null,
    ...over,
  };
}

function withSettings(
  patch: Partial<Settings>,
  over: Partial<DecisionFacts> = {},
) {
  return facts({ settings: { ...SETTINGS_DEFAULTS, ...patch }, ...over });
}

const X32 = { id: "x32", name: "Behringer X32", channels: 32 };
const BUILTIN = { id: "mbp-mic", name: "MacBook Pro Microphone", channels: 1 };

describe("1 — Hvilken lyd?", () => {
  // Den ene raden atlaset ba om ved navn: dagens app maler «Innebygd mikrofon ·
  // Tilkoblet ✓» på vertsstandarden når INGENTING er valgt. Et kort som er
  // grønt fordi en enhet tilfeldigvis finnes er verre enn et som er tomt.
  it("deviceId: null er aldri besvart — heller ikke når enheter finnes", () => {
    const d = decideSound(facts({ devices: [X32, BUILTIN] }));
    expect(d.status).toBe<DecisionStatus>("todo");
    expect(d.answered).toBe(false);
    expect(d.answer).toEqual({ key: "notSetUp" });
    expect(d.detail).toEqual({ key: "noDevice" });
  });

  it("et lagret navn uten id er fortsatt ikke besvart", () => {
    // Den formen finnes i ekte profiler: navnet ble skrevet, id-en ikke.
    const d = decideSound(
      withSettings(
        { deviceId: null, deviceName: "Behringer X32" },
        { devices: [X32] },
      ),
    );
    expect(d.answered).toBe(false);
    expect(d.answer).toEqual({ key: "notSetUp" });
  });

  it("enhetslisten ikke lest ennå ⇒ ingen påstand i noen retning", () => {
    const d = decideSound(
      withSettings(
        { deviceId: "x32", deviceName: "Behringer X32" },
        { devices: null },
      ),
    );
    expect(d.status).toBe<DecisionStatus>("unknown");
    expect(d.answered).toBe(false);
    expect(d.answer).toEqual({
      key: "device",
      name: "Behringer X32",
      pair: null,
    });
    expect(d.detail).toBeNull();
  });

  it("valgt enhet som ikke finnes lenger ⇒ todo, med navnet i teksten", () => {
    const d = decideSound(
      withSettings(
        { deviceId: "x32", deviceName: "Behringer X32" },
        { devices: [BUILTIN] },
      ),
    );
    expect(d.status).toBe<DecisionStatus>("todo");
    expect(d.answer).toEqual({ key: "deviceMissing", name: "Behringer X32" });
    expect(d.detail).toEqual({ key: "deviceGone", name: "Behringer X32" });
  });

  it("valgt enhet som finnes ⇒ done, og navnet er BAKENDENS", () => {
    // Ikke det lagrede: enheten kan ha byttet navn etter en driveroppdatering,
    // og det som står på skjermen skal være det som finnes nå.
    const d = decideSound(
      withSettings(
        { deviceId: "x32", deviceName: "gammelt navn" },
        { devices: [X32] },
      ),
    );
    expect(d.answered).toBe(true);
    expect(d.answer).toEqual({
      key: "device",
      name: "Behringer X32",
      pair: null,
    });
  });

  it("flerkanals enhet med lagret par ⇒ paret står i SVARET, 1-indeksert", () => {
    const d = decideSound(
      withSettings(
        {
          deviceId: "x32",
          deviceName: "Behringer X32",
          deviceChannels: { x32: { channelL: 14, channelR: 15 } },
        },
        { devices: [X32] },
      ),
    );
    expect(d.answer).toEqual({
      key: "device",
      name: "Behringer X32",
      pair: { l: 15, r: 16 },
    });
  });

  it("stereoenhet får aldri et kanalpar — det finnes ikke noe å velge", () => {
    const stereo = { id: "scarlett", name: "Scarlett 2i2", channels: 2 };
    const d = decideSound(
      withSettings(
        {
          deviceId: "scarlett",
          deviceChannels: { scarlett: { channelL: 0, channelR: 1 } },
        },
        { devices: [stereo] },
      ),
    );
    expect(d.answer).toEqual({
      key: "device",
      name: "Scarlett 2i2",
      pair: null,
    });
  });

  it("måleren som hører noe blir detaljen", () => {
    const d = decideSound(
      withSettings({ deviceId: "x32" }, { devices: [X32], vuWord: "hear" }),
    );
    expect(d.detail).toEqual({ key: "heard", word: "hear" });
  });
});

describe("2 — Hvor skal opptakene?", () => {
  it("ingen mappe ⇒ todo", () => {
    const d = decideFolder(facts());
    expect(d.answered).toBe(false);
    expect(d.answer).toEqual({ key: "notSetUp" });
    expect(d.detail).toEqual({ key: "noFolder" });
  });

  it("bare mellomrom er ingen mappe", () => {
    const d = decideFolder(withSettings({ saveFolder: "   " }));
    expect(d.answer).toEqual({ key: "notSetUp" });
  });

  it("mappe uten svar fra disken ⇒ ingen påstand om plass", () => {
    const d = decideFolder(
      withSettings({ saveFolder: "/Users/f/Opptak" }, { diskFreeBytes: null }),
    );
    expect(d.status).toBe<DecisionStatus>("unknown");
    expect(d.answered).toBe(false);
    expect(d.detail).toBeNull();
  });

  it("mappe + ledig plass ⇒ done, med tallene", () => {
    const d = decideFolder(
      withSettings(
        { saveFolder: "/Users/f/Opptak" },
        { diskFreeBytes: 412_000_000_000, roomMinutes: 18_000 },
      ),
    );
    expect(d.answered).toBe(true);
    expect(d.answer).toEqual({ key: "path", path: "/Users/f/Opptak" });
    expect(d.detail).toEqual({
      key: "space",
      freeBytes: 412_000_000_000,
      roomMinutes: 18_000,
    });
  });
});

describe("3 — Hvilken kvalitet?", () => {
  it("standardene (mp3 · 256) er «God»", () => {
    expect(qualityIdFor(SETTINGS_DEFAULTS)).toBe("mp3");
    const d = decideQuality(facts());
    expect(d.answered).toBe(true);
    expect(d.answer).toEqual({ key: "quality", format: "mp3" });
    expect(d.detail).toEqual({ key: "qualityDesc", format: "mp3" });
  });

  it.each([
    ["flac", "flac"],
    ["wav", "wav"],
  ] as const)("%s er ett av kortene", (format, id) => {
    expect(qualityIdFor({ ...SETTINGS_DEFAULTS, format })).toBe(id);
  });

  it("bitraten teller: mp3 · 320 er ikke «God»", () => {
    const s = { ...SETTINGS_DEFAULTS, format: "mp3" as const, bitrate: "320" };
    expect(qualityIdFor(s)).toBeNull();
    const d = decideQuality(facts({ settings: s }));
    // Fortsatt besvart — men kortet sier hva det ER, ikke hva vi skulle ønske.
    expect(d.answered).toBe(true);
    expect(d.answer).toEqual({
      key: "qualityCustom",
      format: "MP3",
      bitrate: "320",
    });
    expect(d.detail).toEqual({ key: "qualityCustomDesc" });
  });

  it("bitraten teller IKKE for flac og wav", () => {
    // De er tapsfrie; `bitrate` er en rest fra mp3-veien og skal ikke gjøre
    // en FLAC-profil «egendefinert».
    expect(
      qualityIdFor({ ...SETTINGS_DEFAULTS, format: "flac", bitrate: "128" }),
    ).toBe("flac");
  });
});

describe("4 — Hvilken kirke?", () => {
  it("tomt navn ⇒ todo, men språket vises likevel", () => {
    const d = decideChurch(withSettings({ churchName: "  " }));
    expect(d.answered).toBe(false);
    expect(d.answer).toEqual({ key: "notSetUp" });
    expect(d.detail).toEqual({ key: "language", language: "no" });
  });

  it("navn ⇒ done", () => {
    const d = decideChurch(
      withSettings({ churchName: "Bryn menighet" }, { locale: "en" }),
    );
    expect(d.answered).toBe(true);
    expect(d.answer).toEqual({ key: "church", name: "Bryn menighet" });
    expect(d.detail).toEqual({ key: "language", language: "en" });
  });

  it("språket er det som RENDRES, ikke det som står lagret", () => {
    // Slik så det ut mens fem kataloger var pauset: en profil satt til tysk
    // leste engelsk. Alle sju er aktive nå (F2-S6), så de to følger hverandre
    // i praksis — men påstanden er om hvilken KILDE kortet leser, og den kan
    // skille lag igjen (en pause, en katalog som ikke lastet). Kortet skal si
    // det brukeren faktisk ser.
    const d = decideChurch(
      withSettings(
        { churchName: "Bryn menighet", language: "de" },
        { locale: "en" },
      ),
    );
    expect(d.detail).toEqual({ key: "language", language: "en" });
  });
});

describe("5 — Hvem får beskjed?", () => {
  it("varsler slått på i OS-et ⇒ den som står ved maskinen, besvart", () => {
    const d = decideNotify(facts({ notificationPermission: "granted" }));
    expect(d.answered).toBe(true);
    expect(d.status).toBe<DecisionStatus>("done");
    expect(d.answer).toEqual({ key: "onMachine" });
    expect(d.detail).toEqual({ key: "onMachineDesc" });
    expect(needsSetUp(d)).toBe(false);
  });

  it("varsler slått av i OS-et ⇒ gult, og «Sett opp»", () => {
    // Et feilvarsel ingen ser, er ingen beskjed.
    const d = decideNotify(facts({ notificationPermission: "denied" }));
    expect(d.answered).toBe(false);
    expect(d.status).toBe<DecisionStatus>("todo");
    expect(d.answer).toEqual({ key: "notificationsOff" });
    expect(d.detail).toEqual({ key: "notificationsOffDesc" });
    expect(needsSetUp(d)).toBe(true);
  });

  it("plattformen kan ikke svare ⇒ besvart, med testvarselet som bevis", () => {
    // macOS i dag. Appen varsler; kortet sier hvordan man ser det selv.
    const d = decideNotify(facts({ notificationPermission: "unknown" }));
    expect(d.answered).toBe(true);
    expect(d.answer).toEqual({ key: "onMachine" });
    expect(d.detail).toEqual({ key: "onMachineUnverifiedDesc" });
  });

  it("ikke spurt ennå ⇒ ingen påstand i noen retning", () => {
    const d = decideNotify(facts());
    expect(d.status).toBe<DecisionStatus>("unknown");
    expect(d.answered).toBe(false);
    expect(needsSetUp(d)).toBe(false);
  });

  it("start-/stopp-bryteren endrer ikke svaret — feil varsles uansett", () => {
    const d = decideNotify(
      withSettings(
        { notifyStart: false, notifyStop: false },
        { notificationPermission: "granted" },
      ),
    );
    expect(d.answered).toBe(true);
    expect(d.answer).toEqual({ key: "onMachine" });
  });
});

describe("de fem sammen", () => {
  it("en fabrikkfersk app har svart på nøyaktig TO — kvalitet og varsling", () => {
    const all = decisionsFor(
      facts({
        devices: [],
        diskFreeBytes: null,
        notificationPermission: "granted",
      }),
    );
    expect(all.map((d) => d.id)).toEqual([
      "sound",
      "folder",
      "quality",
      "church",
      "notify",
    ]);
    expect(answeredCount(all)).toBe(2);
    expect(all.find((d) => d.id === "quality")?.answered).toBe(true);
    expect(all.find((d) => d.id === "notify")?.answered).toBe(true);
  });

  it("en ferdig satt opp app har svart på alle fem", () => {
    const all = decisionsFor(
      withSettings(
        {
          deviceId: "x32",
          deviceName: "Behringer X32",
          saveFolder: "/Users/f/Opptak",
          churchName: "Bryn menighet",
        },
        {
          devices: [X32],
          diskFreeBytes: 412_000_000_000,
          roomMinutes: 18_000,
          notificationPermission: "granted",
        },
      ),
    );
    expect(answeredCount(all)).toBe(5);
  });
});

describe("needsSetUp — «Sett opp» eller «Endre»", () => {
  it("«Sett opp» bare når det ikke står et svar", () => {
    expect(needsSetUp(decideFolder(facts()))).toBe(true);
    expect(
      needsSetUp(decideNotify(facts({ notificationPermission: "granted" }))),
    ).toBe(false);
  });

  it("en mappe uten diskssvar er noe man ENDRER, ikke setter opp", () => {
    // `unknown` er ikke besvart — men det STÅR en sti der, og «Sett opp» på
    // noe som allerede er satt opp beskriver skjermen feil.
    const d = decideFolder(
      withSettings({ saveFolder: "/Users/f/Opptak" }, { diskFreeBytes: null }),
    );
    expect(d.answered).toBe(false);
    expect(needsSetUp(d)).toBe(false);
  });

  it("kvalitet er alltid noe man endrer", () => {
    expect(needsSetUp(decideQuality(facts()))).toBe(false);
  });
});

describe("channelPairs", () => {
  it("gir venstre kanal i hvert par, 0-indeksert", () => {
    expect(channelPairs(8)).toEqual([0, 2, 4, 6]);
  });

  it("lar en odde siste kanal falle ut — den har ingen partner", () => {
    expect(channelPairs(5)).toEqual([0, 2]);
  });

  it("en stereo- eller monoenhet har ingen par å velge mellom", () => {
    expect(channelPairs(2)).toEqual([0]);
    expect(channelPairs(1)).toEqual([]);
    expect(channelPairs(0)).toEqual([]);
  });
});

describe("channelPairFor", () => {
  it("gir null uten lagret kartlegging", () => {
    expect(channelPairFor(SETTINGS_DEFAULTS, "x32")).toBeNull();
  });

  it("legger til 1 på begge — brukeren teller fra 1", () => {
    expect(
      channelPairFor(
        {
          ...SETTINGS_DEFAULTS,
          deviceChannels: { x32: { channelL: 0, channelR: 1 } },
        },
        "x32",
      ),
    ).toEqual({ l: 1, r: 2 });
  });
});
