import { describe, expect, it } from "vitest";

import no from "../../../../legacy/locales/no.json";
import {
  FINDING_SLUGS,
  authSlug,
  findingSlug,
  micOk,
  statusRows,
  storedDeviceFound,
  testErrorSlug,
  testSignalSlug,
  type DiagnoseFacts,
} from "./diagnose-core";

/**
 * Kodene `crates/sundayrec-core/src/diagnostics.rs` faktisk sender, skrevet av
 * for hånd fra `detect_issues`.
 *
 * ⚠️ Denne lista er avskriften, ikke kilden — poenget er nettopp at den er
 * skrevet UAVHENGIG av `FINDING_SLUGS`, så en kode som forsvinner fra tabellen
 * ikke også forsvinner fra fasiten den måles mot.
 */
const RUST_CODES = [
  "SR-FFMPEG-01",
  "SR-AUDIO-01",
  "SR-AUDIO-02",
  "SR-AUDIO-10",
  "SR-RATE-01",
  "SR-VIDEO-01",
  "SR-VIDEO-02",
  "SR-DISK-01",
  "SR-DISK-02",
  "SR-PERM-01",
  "SR-PERM-02",
  "SR-ENGINE-01",
  "SR-CAPTURE-01",
  "SR-CAPTURE-02",
  "SR-CRASH-01",
  "SR-TASK-01",
  "SR-LOG-01",
  "SR-LOG-02",
  "SR-OK",
  "REC-LOSS",
] as const;

/** Katalogens `app.diagnose`-subtre, for oppslag uten i18n-runtime. */
const CAT = (no as { app: { diagnose: Record<string, unknown> } }).app.diagnose;

function lookup(path: string): unknown {
  return path
    .split(".")
    .reduce<unknown>(
      (node, part) => (node as Record<string, unknown> | undefined)?.[part],
      CAT,
    );
}

describe("kode → nøkkel", () => {
  it.each(RUST_CODES)("%s har en nøkkel, og nøkkelen har tekst", (code) => {
    const slug = findingSlug(code);
    expect(slug, `${code} mangler i FINDING_SLUGS`).not.toBeNull();
    // Begge halvdelene, fordi begge slås opp: en tittel uten et råd ville gitt
    // tom tekst nøyaktig der handlingen står.
    expect(typeof lookup(`f.${slug}.title`)).toBe("string");
    expect(typeof lookup(`f.${slug}.hint`)).toBe("string");
  });

  it("dekker nøyaktig kodene Rust sender — verken flere eller færre", () => {
    // Flere ville betydd en nøkkel ingen leser (og en `--unused`-gate som
    // feller den); færre betyr en kode som stille faller på Rust-prosaen.
    expect(Object.keys(FINDING_SLUGS).sort()).toEqual([...RUST_CODES].sort());
  });

  it("SR-OK er en kode som alle andre", () => {
    // Den er lett å glemme fordi den ikke er et problem — og «alt i orden» på
    // norsk til en engelsk bruker er nøyaktig like galt som en feilmelding er.
    expect(findingSlug("SR-OK")).toBe("ok");
  });

  it("en ukjent kode svarer null, så kallstedet faller på motorens prosa", () => {
    expect(findingSlug("SR-NOPE-99")).toBeNull();
    expect(findingSlug("")).toBeNull();
    expect(findingSlug("sr-audio-01")).toBeNull(); // koder er versaler
  });
});

describe("den lagrede enheten mot de enumererte", () => {
  it("treffer på uklart navn, begge veier", () => {
    expect(storedDeviceFound("Qu-5", ["Qu-5 (2- USB Audio)"])).toBe(true);
    expect(storedDeviceFound("Qu-5 (2- USB Audio)", ["Qu-5"])).toBe(true);
    expect(storedDeviceFound("QU-5", ["qu-5"])).toBe(true);
  });

  it("sier nei når enheten ikke er der", () => {
    expect(storedDeviceFound("Qu-5", ["MacBook Pro-mikrofon"])).toBe(false);
    expect(storedDeviceFound("Qu-5", [])).toBe(false);
  });
});

describe("mikrofontilgangen er tre-tilstands", () => {
  it.each([
    ["authorized", true],
    ["denied", false],
    ["restricted", false],
    ["notDetermined", null],
    ["unknown", null],
    [null, null],
  ] as Array<[string | null, boolean | null]>)("%s → %s", (status, want) => {
    expect(micOk(status)).toBe(want);
  });

  it.each([
    ["authorized", "authorized"],
    ["denied", "denied"],
    ["restricted", "restricted"],
    ["notDetermined", "notDetermined"],
    ["unknown", "unknown"],
    [null, "unknown"],
    ["noe-nytt-fra-en-nyere-bakende", "unknown"],
  ] as Array<[string | null, string]>)(
    "nøkkelen for %s er %s",
    (status, want) => {
      expect(authSlug(status)).toBe(want);
      // Gulvet er det som gjør `tDyn` trygg: den kaster i DEV på en bom.
      expect(typeof lookup(`v.auth.${authSlug(status)}`)).toBe("string");
    },
  );
});

describe("statusradene", () => {
  const BASE: DiagnoseFacts = {
    inputs: ["Qu-5", "MacBook Pro-mikrofon"],
    storedDevice: "Qu-5",
    micStatus: "authorized",
    ffmpeg: { available: true, version: "ffmpeg version 7.1" },
    captureOk: true,
    probeSkipped: null,
  };

  it("er alltid fem, i fast rekkefølge", () => {
    expect(statusRows(BASE).map((r) => r.id)).toEqual([
      "devices",
      "selected",
      "mic",
      "engine",
      "probe",
    ]);
    // Også når absolutt ingenting er kjent.
    expect(
      statusRows({
        inputs: [],
        storedDevice: null,
        micStatus: null,
        ffmpeg: null,
        captureOk: null,
        probeSkipped: "video is off",
      }),
    ).toHaveLength(5);
  });

  it("en frisk maskin er fem ✓ (unntatt der ✓ ikke finnes)", () => {
    expect(statusRows(BASE).map((r) => r.tone)).toEqual([
      true,
      true,
      true,
      true,
      true,
    ]);
  });

  it("teller enhetene, og et null-antall er et kryss", () => {
    expect(statusRows(BASE)[0]).toMatchObject({ tone: true, valueText: "2" });
    expect(statusRows({ ...BASE, inputs: [] })[0]).toMatchObject({
      tone: false,
      valueText: "0",
    });
  });

  it("beholder enhetsnavnet når enheten IKKE ble funnet", () => {
    const row = statusRows({ ...BASE, inputs: ["Noe helt annet"] })[1];
    // Navnet er den ene opplysningen som gjør feilen mulig å rette; «ikke
    // funnet» kommer i TILLEGG, ikke i stedet for.
    expect(row).toMatchObject({
      tone: false,
      valueSlug: "notFound",
      valueText: "Qu-5",
    });
  });

  it("ingen lagret enhet er «standardenhet», ikke en feil", () => {
    expect(statusRows({ ...BASE, storedDevice: null })[1]).toMatchObject({
      tone: null,
      valueSlug: "defaultDevice",
    });
    // Blanke tegn er heller ikke et enhetsvalg.
    expect(statusRows({ ...BASE, storedDevice: "   " })[1]).toMatchObject({
      tone: null,
      valueSlug: "defaultDevice",
    });
  });

  it("viser ffmpeg-versjonen når den finnes, og sier fra når motoren mangler", () => {
    expect(statusRows(BASE)[3]).toMatchObject({
      tone: true,
      valueSlug: null,
      valueText: "ffmpeg version 7.1",
    });
    expect(
      statusRows({ ...BASE, ffmpeg: { available: true, version: null } })[3],
    ).toMatchObject({ tone: true, valueSlug: "ffmpegOk" });
    expect(
      statusRows({ ...BASE, ffmpeg: { available: false, version: null } })[3],
    ).toMatchObject({ tone: false, valueSlug: "ffmpegMissing" });
    // Proben svarte ikke i det hele tatt: «vet ikke», ikke «mangler».
    expect(statusRows({ ...BASE, ffmpeg: null })[3]).toMatchObject({
      tone: null,
      valueSlug: "unknown",
    });
  });

  it.each([
    [true, true, "probeOk"],
    [false, false, "probeFail"],
    [null, null, "probeSkipped"],
  ] as Array<[boolean | null, boolean | null, string]>)(
    "captureOk %s → tone %s / %s",
    (captureOk, tone, slug) => {
      const row = statusRows({ ...BASE, captureOk })[4];
      expect(row.tone).toBe(tone);
      expect(row.valueSlug).toBe(slug);
    },
  );

  it("en hoppet-over probe er IKKE et kryss, uansett hva videoen gjorde", () => {
    // ⚠️ Den viktigste av de tre: en probe som ikke kjørte og en probe som
    // fikk stillhet er to helt forskjellige svar, og bare det ene er en feil.
    const skipped = statusRows({
      ...BASE,
      captureOk: null,
      probeSkipped: "another client holds the device",
    })[4];
    expect(skipped.tone).toBeNull();
    expect(skipped.tone).not.toBe(false);
  });

  it("hver verdinøkkel radene peker på finnes i katalogen", () => {
    const combos: DiagnoseFacts[] = [
      BASE,
      { ...BASE, inputs: [], storedDevice: null, captureOk: null },
      { ...BASE, storedDevice: "borte", ffmpeg: null, micStatus: "denied" },
      {
        ...BASE,
        ffmpeg: { available: false, version: null },
        captureOk: false,
      },
    ];
    for (const facts of combos) {
      for (const row of statusRows(facts)) {
        if (row.valueSlug)
          expect(typeof lookup(`v.${row.valueSlug}`)).toBe("string");
        expect(typeof lookup(`r.${row.id}`)).toBe("string");
      }
    }
  });
});

describe("test-opptakets koder", () => {
  it.each([
    ["device_not_found", "deviceNotFound"],
    ["device_permission_denied", "permissionDenied"],
    ["ffmpeg_error", "ffmpegError"],
    ["no_audio", "noAudio"],
  ])("%s → %s, og nøkkelen har tekst", (code, slug) => {
    expect(testErrorSlug(code)).toBe(slug);
    expect(typeof lookup(`testErr.${slug}`)).toBe("string");
  });

  it.each([
    ["silent", "silent"],
    ["low", "low"],
    ["normal", "normal"],
  ])("signal %s → %s", (code, slug) => {
    expect(testSignalSlug(code)).toBe(slug);
    expect(typeof lookup(`testSig.${slug}`)).toBe("string");
  });

  it("ukjente varianter svarer null, så den rå koden vises", () => {
    expect(testErrorSlug("something_new")).toBeNull();
    expect(testErrorSlug(null)).toBeNull();
    expect(testErrorSlug(undefined)).toBeNull();
    expect(testSignalSlug("blazing")).toBeNull();
    expect(testSignalSlug(null)).toBeNull();
  });
});
