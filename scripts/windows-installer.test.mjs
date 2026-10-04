// Windows-installasjonens oppsett som ikke kan bevises lokalt (ingen Windows-
// bygg på Mac-en) men KAN holdes fast: konfigen og release-workflowen sier det
// vi bestemte 2026-10-04 (docs/NEEDS-RICHARD.md §«Eierbeslutninger fra F2»).
//
// Disse testene beviser ikke at installereren virker — det gjør bare
// release-bygget og riggen. De sørger for at ingen i det stille fjerner
// bryterne som de to beslutningene består av:
//
//  - F-W7: NSIS er den installeren nye brukere får, og den installerer for
//    gjeldende bruker (ingen UAC). `.msi` bygges fortsatt, men bare for at en
//    installasjon som allerede kom fra den skal fortsette å oppdatere seg.
//  - `webviewInstallMode: embedBootstrapper`: WebView2-bootstrapperen ligger i
//    installereren i stedet for å lastes ned først.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const read = (rel) =>
  readFileSync(new URL(`../${rel}`, import.meta.url), "utf8");

const conf = JSON.parse(read("src-tauri/tauri.conf.json"));
const workflow = read(".github/workflows/release.yml");

describe("tauri.conf.json bundle.windows", () => {
  it("bygger WebView2-bootstrapperen inn i installereren", () => {
    expect(conf.bundle.windows.webviewInstallMode).toEqual({
      type: "embedBootstrapper",
    });
  });

  it("NSIS installerer for gjeldende bruker, uten administratorrettigheter", () => {
    // `currentUser` er Tauris standard, men står her med vilje: det er hele
    // poenget med F-W7, og en standard kan endre seg uten at noen ser det.
    expect(conf.bundle.windows.nsis.installMode).toBe("currentUser");
  });

  it("bygger NSIS (bundle.targets er 'all' eller nevner nsis)", () => {
    const t = conf.bundle.targets;
    expect(t === "all" || (Array.isArray(t) && t.includes("nsis"))).toBe(true);
  });
});

describe("release.yml, Windows-oppdateringsfeeden", () => {
  it("lar den generiske nøkkelen windows-x86_64 peke på NSIS, ikke MSI", () => {
    // Uten bryteren velger tauri-action MSI når begge pakker finnes (v0.25.0).
    expect(workflow).toMatch(/^\s+updaterJsonPreferNsis:\s*true\s*$/m);
  });

  it("bygger fortsatt begge pakkene på stable (ingen --bundles-bryter utenom for beta)", () => {
    // `--bundles nsis` skal bare gjelde Windows-betaer. Hvis den også traff
    // stable, forsvant `windows-x86_64-msi` fra manifestet og en
    // MSI-installasjon falt i det stille over på NSIS-installereren; promote-
    // release.mjs nekter å promotere et slikt manifest (se STABLE_ONLY_PLATFORMS).
    const m = workflow.match(/^\s+args:\s*(\$\{\{[^\n]*)$/m);
    expect(m).not.toBeNull();
    expect(m[1]).toContain("contains(github.ref_name, '-beta.')");
    expect(m[1]).toContain("'--bundles nsis '");
  });
});
