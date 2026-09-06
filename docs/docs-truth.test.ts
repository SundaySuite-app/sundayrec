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

  // F2-T4: runbooken beskrev «Sett opp» i sjekklista som en knapp som TAR DEG
  // til kortet på Opptak. Etter T4 folder raden ut på stedet og navigerer
  // ingen steder — og nettopp denne setningen er den slags som blir stående i
  // en runbook lenge etter at skjermen sluttet å gjøre det den sier, fordi
  // ingen leser runbooken før neste riggdag.
  it("SMOKE-TEST.md sier at sjekklistas rader folder ut på stedet (F2-T4)", () => {
    // Linjeskiftene er prettiers, ikke forfatterens: en påstand som brakk
    // fordi en setning ble ombrutt ville vært en gate om tekstbredde.
    const text = readDoc("docs/SMOKE-TEST.md").replace(/\s+/g, " ");
    expect(text).toContain("opens the screen in place, in the row");
    expect(text).not.toMatch(/checklist[^.]*takes you to the control room/i);
  });
});
