/**
 * Dokgate — et lite knippe påstander i docs/ som ikke får bli usanne igjen.
 *
 * SundayRec sine docs er uvanlig ærlige: PRIVACY.md sier «ett unntak» og
 * mener det, DISTRIBUTION.md sier «notarisering er deaktivert» og mener det.
 * Det gjør en glemt setning FARLIGERE her enn i en app med vanlig
 * markedsførings-prosa — leseren har lært seg at hver påstand i disse filene
 * er sann, og lar vaktsomheten falle. En løgn ingen forventer å bli løyet
 * til, er den som overlever lengst.
 *
 * Denne testen fanger ikke alt som kan bli usant — bare setningene som
 * FAKTISK rakk å bli usanne (F1-DOCS-1, 2026-09): PRIVACY.md fortsatte
 * å love innlogging med Sunday-konto etter at funksjonen var slettet;
 * CONTRIBUTING.md beskrev SundayRec som en app som transkriberer og strømmer
 * lenge etter at begge deler var fjernet; DISTRIBUTION.md pekte på harde
 * linjenumre inn i `release.yml` (som drifter for hver redigering) og på en
 * tagg som «nyeste» (som slutter å stemme ved neste utgivelse).
 *
 * F1-DOCS-2, 2026-09: samme råteklasse dukket opp i to filer til som
 * DOKGATEN ikke dekket ennå — NEEDS-RICHARD.md og RELEASE-CHECKLIST.md pekte
 * begge på harde `release.yml:NNN`-linjer for notariseringsoppsettet, og
 * D1s omskriving av selve mekanismen (repo-variabelen `NOTARIZE_MAC` i stedet
 * for tre utkommenterte linjer) gjorde referansene FEIL, ikke bare skjøre.
 * Begge peker nå på navngitte markører (`[notarization]`,
 * `[notarize-switch]`) i `release.yml` i stedet. Oppdag du en ny stale
 * setning et annet sted i docs/, legg til et nytt assert her — ikke bare
 * rett teksten og gå videre.
 *
 * F2-DOCS-1, 2026-09: F2s Windows/lydkjede-runde gjorde sju setninger i fire
 * filer usanne. DISTRIBUTION.md påsto at auto-update-feeden var «verified in
 * prod» uten forbehold, og at notarisering var det ENESTE utestående
 * release-gapet — begge glemte at F2-W1 (#243) fant en Windows-only bug
 * (jobbvernet drepte oppdaterings-installereren) som fortsatt er
 * rigg-uverifisert. PRO-AUDIO-WINDOWS.md påsto at macOS var «upåvirket» og
 * fortsatt gikk «via ffmpeg», og at WASAPI/ASIO «begge piper rå PCM inn i
 * ffmpeg-sidecaren» — begge ble usanne 2026-08-01 da lyd-only-opptak (på
 * BEGGE plattformer) flyttet til den native cpal→WAV-motoren og lot
 * ffmpeg-pipen leve videre bare for video-økter. ASIO-TEST-MATRIX.md hadde
 * samme feil to steder: capture-stien er ikke lenger ubetinget
 * «cpal-stream → ffmpeg-pipe», og en vanlig lyd-only-opptak logger ikke
 * lenger `cpal capture starting` (det navnet gjelder video-økter nå).
 * APP-SHELL.md sin reachability-liste listet `recording_status` og
 * `recording_scheduled_stop_ms` som «gettere uten leser ennå» lenge etter at
 * F2-T1 (#236) slettet den første kommandoen og koblet opp leseren for den
 * andre.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const REPO_ROOT = resolve(import.meta.dirname, "..");

const readDoc = (relativePath: string): string =>
  readFileSync(resolve(REPO_ROOT, relativePath), "utf8");

describe("docs-truth", () => {
  it("PRIVACY.md lover ikke lenger innlogging med Sunday-konto (slettet i R1, V1/PR3)", () => {
    expect(readDoc("PRIVACY.md")).not.toContain("Sunday-konto");
  });

  it("CONTRIBUTING.md påstår ikke lenger at appen transkriberer (fjernet i R2)", () => {
    expect(readDoc("CONTRIBUTING.md")).not.toContain("transcribes");
  });

  it('DISTRIBUTION.md peker ikke på harde release.yml-linjenumre eller en fastfrosset "newest tag"', () => {
    const text = readDoc("docs/DISTRIBUTION.md");
    expect(text).not.toMatch(/release\.yml:\d+/);
    expect(text).not.toContain("is the newest tag");
  });

  it("NEEDS-RICHARD.md peker ikke på harde release.yml-linjenumre (F1-DOCS-2)", () => {
    expect(readDoc("docs/NEEDS-RICHARD.md")).not.toMatch(/release\.yml:\d+/);
  });

  it("RELEASE-CHECKLIST.md peker ikke på harde release.yml-linjenumre (F1-DOCS-2)", () => {
    expect(readDoc("docs/RELEASE-CHECKLIST.md")).not.toMatch(
      /release\.yml:\d+/,
    );
  });

  it('DISTRIBUTION.md påstår ikke at auto-update-feeden er "verified in prod" uten forbehold (F2-DOCS-1) — Windows-siden er uverifisert til F2-W1/#243 er riggtestet', () => {
    expect(readDoc("docs/DISTRIBUTION.md")).not.toContain(
      "feed verified in prod",
    );
  });

  it("DISTRIBUTION.md påstår ikke at notarisering er det ENESTE utestående release-gapet (F2-DOCS-1) — F2-W1/#243 er også uverifisert på ekte Windows", () => {
    expect(readDoc("docs/DISTRIBUTION.md")).not.toContain(
      "Nothing here remains to set up;",
    );
  });

  it('PRO-AUDIO-WINDOWS.md påstår ikke at macOS fortsatt går "via ffmpeg" ubetinget (F2-DOCS-1) — lyd-only byttet til native cpal→WAV 2026-08-01', () => {
    expect(readDoc("docs/PRO-AUDIO-WINDOWS.md")).not.toContain(
      "Core Audio (via ffmpeg)",
    );
  });

  it("PRO-AUDIO-WINDOWS.md påstår ikke at WASAPI/ASIO begge ubetinget piper PCM inn i ffmpeg (F2-DOCS-1) — lyd-only skriver rett til WAV nå", () => {
    expect(readDoc("docs/PRO-AUDIO-WINDOWS.md")).not.toContain(
      "Begge piper rå PCM inn i ffmpeg-sidecaren",
    );
  });

  it("ASIO-TEST-MATRIX.md påstår ikke at capture-stien ubetinget er cpal-stream → ffmpeg-pipe (F2-DOCS-1) — kun video-økter piper inn i ffmpeg nå", () => {
    expect(readDoc("docs/ASIO-TEST-MATRIX.md")).not.toContain(
      "cpal-stream → ffmpeg-pipe",
    );
  });

  it('ASIO-TEST-MATRIX.md sier ikke at et lyd-only WASAPI-opptak logger "cpal capture starting" (F2-DOCS-1) — det navnet gjelder video-økter nå', () => {
    expect(readDoc("docs/ASIO-TEST-MATRIX.md")).not.toContain(
      "cpal capture starting host=WASAPI",
    );
  });

  it("APP-SHELL.md lister ikke recording_status/recording_scheduled_stop_ms som uleste gettere (F2-DOCS-1) — F2-T1/#236 slettet den første og koblet opp den andre", () => {
    const text = readDoc("docs/APP-SHELL.md");
    const uleseGettere = text.match(
      /\*\*Gettere uten leser ennå:\*\*[\s\S]*?(?=\n- )/,
    )?.[0];
    expect(uleseGettere).toBeDefined();
    expect(uleseGettere).not.toContain("recording_status");
    expect(uleseGettere).not.toContain("recording_scheduled_stop_ms");
  });
});
