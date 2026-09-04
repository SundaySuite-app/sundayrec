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
  notifyGateStatus,
  qualityIdFor,
  relayGateStatus,
  type DecisionFacts,
  type DecisionStatus,
} from "./decisions-core";
import type { Settings } from "../../state/settings";
import type { RelaySubscriptionStatus } from "@legacy/bindings/RelaySubscriptionStatus";

/** Fabrikkfersk profil + det raden faktisk handler om. */
function facts(over: Partial<DecisionFacts> = {}): DecisionFacts {
  return {
    settings: { ...SETTINGS_DEFAULTS },
    devices: null,
    diskFreeBytes: null,
    roomMinutes: null,
    emailTransport: null,
    // Fabrikkfersk maskin: ingen påmelding hos reléet. `null` ville betydd
    // «ikke lest ennå», og gjort hver rad i denne fila til en `unknown`.
    relayConfirmed: false,
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
    // En profil satt til tysk leser engelsk gjennom redesignet (fem kataloger
    // er pauset). Kortet skal si det brukeren faktisk ser.
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
  const READY = {
    emailOnError: true,
    emailAddress: "lyd@brynmenighet.no",
  } satisfies Partial<Settings>;

  it("fabrikkfersk ⇒ ingen får beskjed", () => {
    const d = decideNotify(facts({ emailTransport: false }));
    expect(d.answered).toBe(false);
    expect(d.answer).toEqual({ key: "nobody" });
    expect(d.detail).toEqual({ key: "nobodyDesc" });
  });

  it("maskinvarsler alene gjør det ikke besvart", () => {
    // Dette er hele poenget med spørsmålet: hvis ingen sitter ved maskinen,
    // er et OS-varsel noe ingen ser.
    const d = decideNotify(
      withSettings(
        { notifyStart: true, notifyStop: true },
        { emailTransport: false },
      ),
    );
    expect(d.answered).toBe(false);
    expect(d.answer).toEqual({ key: "nobody" });
  });

  it("adresse uten bryter ⇒ ikke besvart", () => {
    const d = decideNotify(
      withSettings(
        { emailAddress: "lyd@brynmenighet.no", emailOnError: false },
        { emailTransport: true },
      ),
    );
    expect(d.answered).toBe(false);
  });

  it("bryter uten adresse ⇒ ikke besvart", () => {
    const d = decideNotify(
      withSettings(
        { emailOnError: true, emailAddress: "" },
        { emailTransport: true },
      ),
    );
    expect(d.answered).toBe(false);
  });

  it("alt utfylt, men ingen sendevei ⇒ ikke besvart, og teksten sier det", () => {
    const d = decideNotify(withSettings(READY, { emailTransport: false }));
    expect(d.answered).toBe(false);
    expect(d.answer).toEqual({ key: "nobody" });
    expect(d.detail).toEqual({ key: "nobodyDesc" });
  });

  it("sendeveien ikke sjekket ennå ⇒ ingen påstand", () => {
    const d = decideNotify(withSettings(READY, { emailTransport: null }));
    expect(d.status).toBe<DecisionStatus>("unknown");
    expect(d.answered).toBe(false);
  });

  it("adresse + bryter + sendevei ⇒ done", () => {
    const d = decideNotify(withSettings(READY, { emailTransport: true }));
    expect(d.answered).toBe(true);
    expect(d.answer).toEqual({ key: "email", address: "lyd@brynmenighet.no" });
    expect(d.detail).toEqual({ key: "emailDesc" });
  });

  // ── Reléet er den andre sendeveien ────────────────────────────────────────

  it("et bekreftet relé er nok — uten SMTP i det hele tatt", () => {
    // Selve poenget med reléet: kortet blir grønt for en menighet som aldri
    // har sett en SMTP-innstilling.
    const d = decideNotify(
      withSettings(READY, { emailTransport: false, relayConfirmed: true }),
    );
    expect(d.answered).toBe(true);
    expect(d.answer).toEqual({ key: "email", address: "lyd@brynmenighet.no" });
  });

  it("… og den ene JA-en trumfer en sendevei som ikke er lest ennå", () => {
    // Ingen `unknown` her: det finnes en vei ut, og at vi ikke vet noe om den
    // andre endrer ikke det.
    const d = decideNotify(
      withSettings(READY, { emailTransport: null, relayConfirmed: true }),
    );
    expect(d.status).toBe<DecisionStatus>("done");
  });

  it("reléet ikke lest ennå ⇒ ingen påstand, selv om SMTP sa nei", () => {
    const d = decideNotify(
      withSettings(READY, { emailTransport: false, relayConfirmed: null }),
    );
    expect(d.status).toBe<DecisionStatus>("unknown");
    expect(d.answered).toBe(false);
  });

  it("begge lest, begge nei ⇒ «ingen får e-post», og teksten sier hvorfor", () => {
    const d = decideNotify(
      withSettings(READY, { emailTransport: false, relayConfirmed: false }),
    );
    expect(d.answered).toBe(false);
    expect(d.answer).toEqual({ key: "nobody" });
    expect(d.detail).toEqual({ key: "nobodyDesc" });
  });

  it("bryteren AV gjør et bekreftet relé irrelevant", () => {
    // Abonnementet er en sendevei, ikke et samtykke til å bruke den.
    const d = decideNotify(
      withSettings(
        { ...READY, emailOnError: false },
        { relayConfirmed: true, emailTransport: true },
      ),
    );
    expect(d.answered).toBe(false);
  });
});

describe("de fem sammen", () => {
  it("en fabrikkfersk app har svart på nøyaktig ÉN — kvalitet", () => {
    const all = decisionsFor(
      facts({ devices: [], diskFreeBytes: null, emailTransport: false }),
    );
    expect(all.map((d) => d.id)).toEqual([
      "sound",
      "folder",
      "quality",
      "church",
      "notify",
    ]);
    expect(answeredCount(all)).toBe(1);
    expect(all.find((d) => d.id === "quality")?.answered).toBe(true);
  });

  it("en ferdig satt opp app har svart på alle fem", () => {
    const all = decisionsFor(
      withSettings(
        {
          deviceId: "x32",
          deviceName: "Behringer X32",
          saveFolder: "/Users/f/Opptak",
          churchName: "Bryn menighet",
          emailOnError: true,
          emailAddress: "lyd@brynmenighet.no",
        },
        {
          devices: [X32],
          diskFreeBytes: 412_000_000_000,
          roomMinutes: 18_000,
          emailTransport: true,
        },
      ),
    );
    expect(answeredCount(all)).toBe(5);
  });
});

describe("needsSetUp — «Sett opp» eller «Endre»", () => {
  it("«Sett opp» bare når det ikke står et svar", () => {
    expect(needsSetUp(decideFolder(facts()))).toBe(true);
    expect(needsSetUp(decideNotify(facts({ emailTransport: false })))).toBe(
      true,
    );
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

describe("notifyGateStatus", () => {
  const built = {
    featureBuilt: true,
    smtpConfigured: true,
    smtpPasswordAvailable: true,
  };

  it("ikke lest ennå ⇒ åpen — en bryter som er inert i et halvsekund tar ikke imot det første klikket", () => {
    expect(notifyGateStatus(null)).toBe("ok");
  });

  it("uten e-postfeaturen ⇒ «finnes ikke i denne utgaven»", () => {
    // Det er ikke noe brukeren kan gjøre noe med, og å la henne prøve er å be
    // om en lørdagskveld.
    expect(notifyGateStatus({ ...built, featureBuilt: false })).toBe(
      "unavailable",
    );
  });

  it("uten SMTP-vert ⇒ «ikke satt opp», som er noe man KAN gjøre noe med", () => {
    expect(notifyGateStatus({ ...built, smtpConfigured: false })).toBe(
      "unconfigured",
    );
  });

  it("uten passord ⇒ også «ikke satt opp» — vert og brukernavn alene sender ingenting", () => {
    expect(notifyGateStatus({ ...built, smtpPasswordAvailable: false })).toBe(
      "unconfigured",
    );
  });

  it("alt på plass ⇒ åpen, og uten banner", () => {
    expect(notifyGateStatus(built)).toBe("ok");
  });
});

describe("relayGateStatus — reléet er hovedveien, SMTP alternativet", () => {
  const smtp = {
    featureBuilt: true,
    smtpConfigured: true,
    smtpPasswordAvailable: true,
  };
  const noSmtp = {
    featureBuilt: true,
    smtpConfigured: false,
    smtpPasswordAvailable: false,
  };
  const relay = (over: Partial<RelaySubscriptionStatus> = {}) => ({
    endpointBuilt: true,
    state: null,
    address: null,
    enrolledAt: null,
    confirmedAt: null,
    queued: 0,
    ...over,
  });

  it("et bekreftet abonnement er nok — helt uten SMTP", () => {
    // Hele poenget med reléet: en frivillig skal ikke trenge en e-postserver.
    expect(relayGateStatus(noSmtp, relay({ state: "confirmed" }))).toBe("ok");
  });

  it("… og til og med uten e-postfeaturen, for reléet er HTTP", () => {
    expect(
      relayGateStatus(
        { ...noSmtp, featureBuilt: false },
        relay({ state: "confirmed" }),
      ),
    ).toBe("ok");
  });

  it("SMTP alene er fortsatt nok — eksisterende menigheter merker ingenting", () => {
    expect(relayGateStatus(smtp, relay())).toBe("ok");
  });

  it("påmeldt men ikke bekreftet er IKKE en sendevei", () => {
    // Dobbel opt-in: før noen har trykket i innboksen kommer det ingenting
    // fram, og en åpen bryter her ville lovet varsler som ikke sendes.
    expect(relayGateStatus(noSmtp, relay({ state: "pending" }))).toBe(
      "unconfigured",
    );
  });

  it("en adresse som avviser e-post er heller ikke en sendevei", () => {
    expect(relayGateStatus(noSmtp, relay({ state: "suppressed" }))).toBe(
      "unconfigured",
    );
  });

  it("bekreftet på en build UTEN endepunkt er ikke en sendevei", () => {
    // Nedgradering: raden overlever, endepunktet gjør ikke. Uten SMTP er det
    // ingenting igjen, og da er «ikke tilgjengelig» det ærlige svaret.
    expect(
      relayGateStatus(
        { ...noSmtp, featureBuilt: false },
        relay({ endpointBuilt: false, state: "confirmed" }),
      ),
    ).toBe("unavailable");
  });

  it("verken e-postfeature eller endepunkt ⇒ «finnes ikke i denne utgaven»", () => {
    expect(
      relayGateStatus(
        { ...noSmtp, featureBuilt: false },
        relay({ endpointBuilt: false }),
      ),
    ).toBe("unavailable");
  });

  it("uten endepunkt, MEN med e-postfeaturen ⇒ «ikke satt opp»", () => {
    // Det finnes fortsatt noe å gjøre: sett opp en SMTP-server under Avansert.
    expect(relayGateStatus(noSmtp, relay({ endpointBuilt: false }))).toBe(
      "unconfigured",
    );
  });

  it("ikke lest ennå ⇒ åpen, uansett hvilken av de to som mangler", () => {
    expect(relayGateStatus(null, relay({ state: "confirmed" }))).toBe("ok");
    expect(relayGateStatus(noSmtp, null)).toBe("ok");
    expect(relayGateStatus(null, null)).toBe("ok");
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
