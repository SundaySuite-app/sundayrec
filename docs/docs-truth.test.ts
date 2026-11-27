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

  it("PRIVACY.md lover ikke lenger e-post når et opptak feiler (e-postvarslene er fjernet)", () => {
    const text = readDoc("PRIVACY.md");
    expect(text).not.toContain("du kan få e-post");
    expect(text).not.toContain("E-postoppsett");
  });

  it("README.md lister ikke `email` blant standard-featurene (fjernet med e-postvarslene)", () => {
    expect(readDoc("README.md")).not.toMatch(/`updater`, `email`/);
  });

  it("CONTRIBUTING.md påstår ikke lenger at appen transkriberer (fjernet i R2)", () => {
    expect(readDoc("CONTRIBUTING.md")).not.toContain("transcribes");
  });

  it('DISTRIBUTION.md peker ikke på harde release.yml-linjenumre eller en fastfrosset "newest tag"', () => {
    const text = readDoc("docs/DISTRIBUTION.md");
    expect(text).not.toMatch(/release\.yml:\d+/);
    expect(text).not.toContain("is the newest tag");
  });

  it("RELEASE-CHECKLIST.md peker ikke på harde release.yml-linjenumre (F1-DOCS-2)", () => {
    expect(readDoc("docs/RELEASE-CHECKLIST.md")).not.toMatch(
      /release\.yml:\d+/,
    );
  });

  // Beta-følger-stabil (2026-09): scripts/promote-release.mjs's "THE RULE"
  // started letting a plain `vX.Y.Z` tag be promoted to BOTH "stable" and
  // "beta", not just "stable" — an official release must reach beta testers
  // too, so nobody on the beta ring is left running something older than
  // the fleet. §5g is where that second promotion actually happens; pinned
  // here so a future edit to the checklist can't quietly drop it back to
  // "promote to stable only" without this test noticing.
  it("RELEASE-CHECKLIST.md nevner at et rent tagg promoteres til beta òg (2026-09, beta-følger-stabil)", () => {
    expect(readDoc("docs/RELEASE-CHECKLIST.md")).toContain(
      "node scripts/promote-release.mjs beta vX.Y.Z",
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

  // Rent bord (F7): PLAN.md er registeret, og det skal fortelle både hva som er
  // fjernet (så ingen legger det inn igjen) og at appen ikke laster opp noe.
  it("PLAN.md lister det som er bevisst fjernet, og at appen ikke laster opp noe sted", () => {
    const text = readDoc("docs/PLAN.md").replace(/\s+/g, " ");
    expect(text).toContain("Bevisst fjernet i «rent bord»");
    expect(text).toContain("appen laster ikke opp noe sted");
  });

  // F2-T4: runbooken beskrev «Sett opp» i sjekklista som en knapp som TAR DEG
  // til kortet på Opptak. Etter T4 folder raden ut på stedet og navigerer
  // ingen steder — og nettopp denne setningen er den slags som blir stående i
  // en runbook lenge etter at skjermen sluttet å gjøre det den sier, fordi
  // ingen leser runbooken før neste riggdag.
  it("SMOKE-TEST.md beskriver første gang som tre hint på Ta opp (F7, E9)", () => {
    // Linjeskiftene er prettiers, ikke forfatterens: en påstand som brakk
    // fordi en setning ble ombrutt ville vært en gate om tekstbredde.
    const text = readDoc("docs/SMOKE-TEST.md").replace(/\s+/g, " ");
    expect(text).toContain("three hint bubbles sit on **Ta opp**");
    expect(text).not.toMatch(/five real question screens/i);
  });

  // A2, 2026-10: fetch-ffmpeg.mjs gikk til 9.0.1 i v0.21.0 (2026-09-28), men
  // DISTRIBUTION.md sa «Version: 8.1.2» og «Not 9.0» i to uker. Hentescriptet er
  // sannheten (det er det som faktisk havner i installøren), så dokumentet må
  // følge det — og bare den ene av dem kan ikke lenger flyttes uten at denne
  // feiler. Matcher på mønstrene, ikke på et tall, så en vanlig bump (endre
  // begge) går grønt og en halv bump (endre én) går rødt.
  it("DISTRIBUTION.md sier samme ffmpeg-versjon som FFMPEG_VERSION i scripts/fetch-ffmpeg.mjs (A2)", () => {
    const script = readDoc("scripts/fetch-ffmpeg.mjs");
    const pinned = /const FFMPEG_VERSION = "(\d+\.\d+\.\d+)";/.exec(
      script,
    )?.[1];
    const documented = /\*\*Version: (\d+\.\d+\.\d+)\*\*/.exec(
      readDoc("docs/DISTRIBUTION.md"),
    )?.[1];
    expect(
      pinned,
      "FFMPEG_VERSION ikke funnet i fetch-ffmpeg.mjs",
    ).toBeDefined();
    expect(
      documented,
      "«**Version: x.y.z**» ikke funnet i DISTRIBUTION.md",
    ).toBeDefined();
    expect(documented).toBe(pinned);
    // Nedlastingstabellen nevner versjonen også; ingen av dem får henge igjen på en annen.
    const table = readDoc("docs/DISTRIBUTION.md");
    for (const m of table.matchAll(/<buildid>_(\d+\.\d+\.\d+)\//g)) {
      expect(m[1]).toBe(pinned);
    }
  });
});
