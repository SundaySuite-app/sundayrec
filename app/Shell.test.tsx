import { render } from "preact-render-to-string";
import { describe, expect, it } from "vitest";

import type { RecordingEntry } from "@lib/../types";

import { Overlays, Shell } from "./Shell";
import { navigate, route } from "./router/router";
import { setLocale } from "./i18n";
import { recordings } from "./state/recordings";
import { patchSettings } from "./state/settings";

// Fortsatt det ene stedet i enhetsgaten som beviser (a) at `.tsx` kompilerer
// uten et Babel-forvalg — transformen er tsconfigs `jsxImportSource: "preact"`
// — og (b) at `@lib/*` når fram til legacy-rendereren, så det nye skallet
// leser de SAMME sju katalogene den utsendte appen gjør.
//
// S1b legger til det som er nytt: at skinnen faktisk er der, og at hver
// destinasjon viser det som er SANT i stedet for en plassholdertekst.
/** Én historikkrad, i formen `getHistory` faktisk svarer med. */
function row(over: Partial<RecordingEntry> = {}): RecordingEntry {
  return {
    date: "2026-08-23T11:00:00.000Z",
    startTime: "",
    duration: "1t 2m",
    filename: "2026-08-23.mp3",
    path: "/Users/x/SundayRec/2026-08-23.mp3",
    status: "ok",
    timestamp: 1_756_000_000_000,
    durationSec: 3734,
    fileSizeBytes: 112_000_000,
    ...over,
  };
}

describe("Shell", () => {
  it("rendrer skinnen med sidenavnet fra katalogen, ikke fra en literal", () => {
    navigate("record");
    const html = render(<Shell />);
    expect(html).toContain("Opptak");
    expect(html).toContain('data-testid="app-heading"');
    expect(html).toContain('data-testid="nav-record"');
    expect(html).toContain("data-tauri-drag-region");
  });

  it("følger ruten, og språket", async () => {
    navigate("settings", { tab: "settings-audio" });
    expect(render(<Shell />)).toContain("Oppsett");
    await setLocale("en");
    // Sluttet `@lib` å resolve, ville `t()` gitt tom tekst og dette vært en
    // tom `<h1>` — som er nøyaktig hvordan en stille ødelagt alias ser ut.
    expect(render(<Shell />)).toContain("Setup");
    await setLocale("no");
  });

  it("bærer ruten som attributter, ikke som synlig feilsøkingstekst", () => {
    // P1a: kameraet er et TILLEGG på nivå 1, ikke en egen fane. Lenken bærer
    // derfor bare ankeret — og ankeret er kortet den skal lande på.
    navigate("settings", { tab: "settings-video" });
    const html = render(<Shell />);
    expect(html).toContain('data-anchor="camera"');
    expect(html).not.toContain("data-tab=");
    expect(route.value.page).toBe("setup");

    // En fane som FINNES bærer den som et attributt og ingenting annet.
    navigate("settings", { tab: "settings-audio" });
    expect(render(<Shell />)).toContain('data-tab="sound"');
  });

  it("OPPTAK sier fra når ingen lydkilde er valgt — og peker på OPPSETT", () => {
    navigate("record");
    patchSettings({ deviceName: null, deviceId: null });
    const html = render(<Shell />);
    expect(html).toContain('data-testid="record-no-source"');
    expect(html).toContain('data-testid="record-choose-sound"');
  });

  it("OPPTAK viser kilden og måleren når kilden ER valgt", () => {
    navigate("record");
    patchSettings({ deviceName: "Behringer X32", deviceId: "x32" });
    const html = render(<Shell />);
    expect(html).toContain('data-testid="record-source"');
    expect(html).toContain('data-testid="record-vu"');
    expect(html).toContain("Behringer X32");
    expect(html).not.toContain('data-testid="record-no-source"');
    patchSettings({ deviceName: null, deviceId: null });
  });

  it("OPPTAK sperrer Start med en GRUNN, ikke med en grå knapp", () => {
    navigate("record");
    patchSettings({ deviceName: null, deviceId: null });
    const html = render(<Shell />);
    // `aria-disabled`, aldri `disabled`: en tastaturbruker skal kunne komme
    // fram til knappen for å HØRE hvorfor den er av.
    expect(html).toMatch(/data-testid="record-start"[^>]*aria-disabled="true"/);
    expect(html).not.toMatch(/data-testid="record-start"[^>]* disabled/);
    expect(html).toContain("Start er sperret til lyden er valgt");
  });

  it("BIBLIOTEK påstår ingenting før opptakene er talt", () => {
    navigate("library");
    recordings.value = null;
    const html = render(<Shell />);
    // Verken «ingen opptak» eller et antall: vi vet ikke ennå.
    expect(html).not.toContain('data-testid="library-empty"');
    expect(html).not.toContain('data-testid="library-stored"');
  });

  it("BIBLIOTEK viser tomtilstanden bare når det FAKTISK er tomt", () => {
    navigate("library");
    recordings.value = [];
    expect(render(<Shell />)).toContain('data-testid="library-empty"');

    recordings.value = [row(), row(), row()];
    const withRows = render(<Shell />);
    expect(withRows).toContain('data-testid="library-stored"');
    expect(withRows).not.toContain('data-testid="library-empty"');
    recordings.value = null;
  });

  it("OPPSETT viser de fem spørsmålene, med svaret som gjelder nå", () => {
    navigate("setup");
    patchSettings({
      deviceId: "x32",
      deviceName: "Behringer X32",
      saveFolder: "/Users/x/SundayRec",
      format: "mp3",
      churchName: "",
      emailOnError: false,
      emailAddress: "",
    });
    const html = render(<Shell />);
    for (const id of ["sound", "folder", "quality", "church", "notify"]) {
      expect(html).toContain(`data-testid="setup-row-${id}"`);
    }
    // Enhetslisten er ikke lest i en node-render, så spørsmål 1 påstår
    // INGENTING — verken at enheten finnes eller at den er borte. Det er hele
    // poenget med den tredje tilstanden.
    expect(html).toMatch(
      /data-testid="setup-row-sound"[^>]*data-status="unknown"/,
    );
    // Besvart ⇒ nøytral; ubesvart ⇒ gul. Den gule raden er hele grunnen til at
    // noen oppdager den tomme innstillingen før en søndag.
    expect(html).toMatch(/data-testid="setup-row-church"[^>]*data-tone="warn"/);
    expect(html).toContain("Ingen ennå");
    // Og de to tilleggene, som utvider seg når de slås på.
    expect(html).toContain('data-testid="setup-camera"');
    expect(html).toContain('data-testid="setup-auto"');
    patchSettings({ deviceId: null, deviceName: null, saveFolder: null });
  });

  it("OPPSETT/lyd er en EGEN skjerm, med spørsmålet som overskrift", () => {
    // Skinnen står fortsatt på OPPSETT, men `<h1>` er spørsmålet: siden
    // handler om «Hvilken lyd?», og fokus flyttes hit ved hvert rutebytte.
    navigate("settings", { tab: "settings-audio" });
    const html = render(<Shell />);
    expect(html).toMatch(/data-testid="app-heading"[^>]*>Hvilken lyd\?</);
    expect(html).toContain('data-testid="setup-back"');
    navigate("setup");
  });

  it("sier fra når innstillingene ikke kunne leses", () => {
    navigate("record");
    expect(render(<Shell />)).not.toContain('data-testid="hydrate-error"');
  });

  it("Overlays er dialog- og toastverten, og ingenting når begge er tomme", () => {
    expect(render(<Overlays />)).toBe("");
  });

  it("monterer utviklingsproben bare når den blir bedt om det", () => {
    navigate("record");
    expect(render(<Shell />)).not.toContain("setting-probe");
  });
});
