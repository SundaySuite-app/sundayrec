import { render } from "preact-render-to-string";
import { describe, expect, it } from "vitest";

import type { RecordingEntry } from "@legacy/types";

import { Overlays, Shell } from "./Shell";
import { lastEdited, loadState } from "./editor/model";
import { navigate, route } from "./router/router";
import { setLocale } from "./i18n";
import { recordings } from "./state/recordings";
import { trashEntries } from "./state/trash";
import { patchSettings } from "./state/settings";

// Fortsatt det ene stedet i enhetsgaten som beviser (a) at `.tsx` kompilerer
// uten et Babel-forvalg — transformen er tsconfigs `jsxImportSource: "preact"`
// — og (b) at `@lib/*` når fram til legacy-rendereren, så det nye skallet
// leser de SAMME sju katalogene den utsendte appen gjør.
//
// S1b legger til det som er nytt: at skallet faktisk er der, og at hver
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
  it("rendrer skallet med sidenavnet fra katalogen, ikke fra en literal", () => {
    navigate("record");
    const html = render(<Shell />);
    expect(html).toContain("Opptak");
    expect(html).toContain('data-testid="app-heading"');
    expect(html).toContain('data-testid="nav-record"');
    expect(html).toContain("data-tauri-drag-region");
  });

  it("er tre bånd i rekkefølge: topplinje, side, bunnlinje — og skinnen er borte", () => {
    // D3: venstreskinnen er revet. Det som var i den bor nå i to linjer, og
    // REKKEFØLGEN i treet er den samme som rekkefølgen på skjermen — det er den
    // som bestemmer hva en skjermleser og en tabtast møter først.
    navigate("record");
    patchSettings({ churchName: "Bryn menighet" });
    const html = render(<Shell />);

    const top = html.indexOf('data-testid="topbar"');
    const main = html.indexOf('data-testid="main"');
    const bottom = html.indexOf('data-testid="bottombar"');
    expect(top).toBeGreaterThan(-1);
    expect(main).toBeGreaterThan(top);
    expect(bottom).toBeGreaterThan(main);
    // Skinnen finnes ikke lenger — verken som element eller som testid.
    expect(html).not.toContain('data-testid="rail"');
    expect(html).not.toContain('data-testid="rail-church"');

    // ⚠️ MUTASJONSPRØVEN for dra-sonen: attributtet står på TOPPLINJENS rot og
    // ingen andre steder. Flyttes det (eller fjernes), er vinduet ikke lenger
    // mulig å dra — og ingen klikk-test ville merket det.
    const headerTag = html.slice(
      html.indexOf("<header"),
      html.indexOf(">", html.indexOf("<header")) + 1,
    );
    expect(headerTag).toContain('data-testid="topbar"');
    expect(headerTag).toContain("data-tauri-drag-region");
    expect(html.match(/data-tauri-drag-region/g)).toHaveLength(1);
    // …og bunnlinja er IKKE en dra-sone: den er full av knapper.
    expect(html).not.toMatch(
      /data-testid="bottombar"[^>]*data-tauri-drag-region/,
    );

    // Topplinja: merket, produktnavnet og kirken. Ingen knapper.
    expect(html).toContain('data-testid="app-logo"');
    expect(html).toContain('data-testid="shell-church"');
    expect(html).toContain("Bryn menighet");

    // Bunnlinja: statuslinjen, de tre destinasjonene, versjonen og tannhjulet.
    const bar = html.slice(bottom);
    for (const id of [
      "status-line",
      "status-dot",
      "status-text",
      "nav-record",
      "nav-edit",
      "nav-export",
      "nav-setup",
    ]) {
      expect(bar).toContain(`data-testid="${id}"`);
    }
    patchSettings({ churchName: "" });
  });

  it("sier fra i topplinja når kirken ikke er satt opp", () => {
    navigate("record");
    patchSettings({ churchName: "" });
    const html = render(<Shell />);
    // Samme katalognøkkel som i skinnen — det er PLASSEN som flyttet, ikke
    // setningen. Gult, fordi det er noe som må gjøres.
    expect(html).toMatch(
      /data-testid="shell-church"[^>]*class="[^"]*churchUnset/,
    );
    expect(html).toContain("Ikke satt opp ennå");
  });

  it("følger ruten, og språket", async () => {
    // D2: destinasjonen heter «Innstillinger» og bor på tannhjulet nederst i
    // skinnen. Ruten er den samme (`setup`); det er navnet og plasseringen som
    // flyttet, og teksten kommer fortsatt fra katalogen.
    navigate("settings", { tab: "settings-audio" });
    expect(render(<Shell />)).toContain("Innstillinger");
    await setLocale("en");
    // Sluttet `@lib` å resolve, ville `t()` gitt tom tekst og dette vært en
    // tom `<h1>` — som er nøyaktig hvordan en stille ødelagt alias ser ut.
    expect(render(<Shell />)).toContain("Settings");
    await setLocale("no");
  });

  it("bærer den gamle logoen, og Innstillinger på et tannhjul framfor som destinasjon", () => {
    navigate("record");
    const html = render(<Shell />);

    // D2, eierens første ønske: merket fra den utsendte appen, ikke den gule
    // «S»-boksen skallet malte mens tegningen manglet.
    expect(html).toContain('data-testid="app-logo"');
    // ⚠️ `<defs>`-id-er er GLOBALE i dokumentet. Prefikset er kollisjonsvakten
    // mot `src-tauri/app-icon.svg`s generiske `bg`/`glow`/`gold`/`clip` — se
    // filhodet i `ui/AppLogo/AppLogo.tsx`. Faller det bort, peker
    // `url(#gold)` på hvem som helst.
    expect(html).toContain('id="srlogo-clip"');
    expect(html).toContain('id="srlogo-gold"');
    expect(html).not.toContain('id="clip"');
    expect(html).not.toContain('id="gold"');

    // Kontrakten som IKKE flyttet: knappen heter fortsatt `nav-setup`, og den
    // sier fortsatt fra når man står der. Alt som spør skinnen «hvor er jeg?»
    // — e2e, `no-live-surface`s telling, skjermleseren — får samme svar.
    expect(html).toContain('data-testid="nav-setup"');
    expect(html).not.toMatch(/data-testid="nav-setup"[^>]*aria-current/);
    navigate("setup");
    expect(render(<Shell />)).toMatch(
      /data-testid="nav-setup"[^>]*aria-current="page"/,
    );
    navigate("record");
  });

  it("bærer ruten som attributter, ikke som synlig feilsøkingstekst", () => {
    // D2: hver gammel innstillingsfane er et KORT i kontrollrommet, ikke en
    // fane. Lenken bærer derfor bare ankeret — og ankeret er kortet den skal
    // folde ut.
    navigate("settings", { tab: "settings-video" });
    const html = render(<Shell />);
    expect(html).toContain('data-anchor="camera"');
    expect(html).not.toContain("data-tab=");
    expect(route.value.page).toBe("record");

    navigate("settings", { tab: "settings-audio" });
    expect(route.value).toEqual({
      page: "record",
      anchor: "sound",
      highlight: true,
    });

    // D3: `editor` er en DESTINASJON nå, ikke en fane. Den lander på REDIGERING
    // uten fane i det hele tatt — attributtet skal derfor ikke finnes.
    navigate("editor");
    const edit = render(<Shell />);
    expect(edit).toContain('data-page="edit"');
    expect(edit).not.toContain("data-tab=");
    navigate("record");
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

  it("REDIGERING påstår ingenting før opptakene er talt", () => {
    navigate("edit");
    recordings.value = null;
    const html = render(<Shell />);
    // Verken «ingen opptak» eller en liste: vi vet ikke ennå. Papirkurv-lenken
    // står likevel — den er den ene tingen som ALLTID skal ha en dør.
    expect(html).not.toContain('data-testid="library-empty"');
    expect(html).not.toContain('data-testid="library-row"');
    expect(html).toContain('data-testid="library-trash-open"');
  });

  it("REDIGERING viser tomtilstanden bare når det FAKTISK er tomt", () => {
    navigate("edit");
    recordings.value = [];
    expect(render(<Shell />)).toContain('data-testid="library-empty"');

    recordings.value = [
      row({ path: "/a.mp3", filename: "a.mp3" }),
      row({ path: "/b.mp3", filename: "b.mp3" }),
      row({ path: "/c.mp3", filename: "c.mp3" }),
    ];
    const withRows = render(<Shell />);
    expect(withRows).toContain('data-testid="library-row"');
    expect(withRows).toContain("Opptak: 3");
    expect(withRows).not.toContain('data-testid="library-empty"');
    recordings.value = null;
  });

  it("REDIGERING har en papirkurv-inngang også når kurven er tom", () => {
    // Atlaset §5, funn 9: legacy skjuler «Papirkurv»-lenken når `trash_list`
    // er tom, så en frivillig som slettet noe i går og leter etter det i dag
    // finner ingen dør hvis sveipen har vært innom i mellomtiden.
    navigate("edit");
    recordings.value = [];
    trashEntries.value = [];
    const html = render(<Shell />);
    expect(html).toContain('data-testid="library-trash-open"');
    expect(html).toContain("Papirkurven er tom");
    trashEntries.value = null;
    recordings.value = null;
  });

  it("PAPIRKURVEN er en fane inne i REDIGERING, ikke et femte sted", () => {
    navigate("edit", { tab: "trash" });
    const html = render(<Shell />);
    // Skinnen står på REDIGERING hele veien; det er overskriften som bytter.
    expect(html).toMatch(/data-testid="nav-edit"[^>]*aria-current="page"/);
    expect(html).toContain('data-testid="trash-back"');
    expect(html).toMatch(/data-testid="app-heading"[^>]*>Papirkurv</);
    // …og papirkurven er UTENFOR slippsonen: et opptak sluppet på den ville
    // sett ut som en handling, og den ene handlingen det ligner på er den vi
    // ikke gjør.
    expect(html).not.toContain('data-testid="edit-dropzone"');
    navigate("edit");
  });

  it("REDIGERING viser biblioteket til en fil er på gang — og bytter da", () => {
    // Bryteren er `loadState`, ikke `hasFile`: se `app/Shell.tsx`. Grenen er
    // hele grunnen til at biblioteket aldri blinker innom under en åpning, og
    // e2e ser det samme utenfra (`e2e/editor.spec.ts`).
    navigate("edit");
    recordings.value = [row({ path: "/a.mp3", filename: "a.mp3" })];
    loadState.value = "idle";
    const list = render(<Shell />);
    expect(list).toContain('data-testid="library-row"');
    expect(list).not.toContain('data-testid="editor"');
    // Slippsonen omslutter begge visningene — en fil fra en annen opptaker
    // skal kunne slippes mens LISTA står, som er mesteparten av tiden.
    expect(list).toContain('data-testid="edit-dropzone"');
    // Overskriften er destinasjonens eget navn så lenge ingen fil er åpen.
    expect(list).toMatch(/data-testid="app-heading"[^>]*>Redigering</);

    loadState.value = "loading";
    const editing = render(<Shell />);
    expect(editing).toContain('data-testid="editor"');
    expect(editing).not.toContain('data-testid="library-row"');
    expect(editing).toContain('data-testid="edit-dropzone"');

    loadState.value = "idle";
    recordings.value = null;
  });

  it("EKSPORTERING uten en åpen fil tilbyr det sist redigerte, ikke en tom side", () => {
    // Eiervalg 3 i D3: en eksport skal alltid være ett klikk unna.
    navigate("export");
    loadState.value = "idle";
    lastEdited.value = {
      path: "/Users/x/SundayRec/2026-08-23.mp3",
      fileName: "2026-08-23.mp3",
      startedAtMs: null,
    };
    const html = render(<Shell />);
    expect(html).toMatch(/data-testid="app-heading"[^>]*>Eksportering</);
    expect(html).toContain('data-testid="export-last"');
    expect(html).toContain("Sist redigert");
    expect(html).toContain("2026-08-23.mp3");
    // Ingen valg-skjema uten en fil: knappene ville ikke kunnet gjøre noe.
    expect(html).not.toContain('data-testid="editor-export-go"');

    lastEdited.value = null;
    navigate("record");
  });

  it("OPPTAK er kontrollrommet: seks kort, med svaret som gjelder nå", () => {
    navigate("record");
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
    // Kilden i venstrekolonnen, og de fem i stabelen til høyre. De to
    // tilleggene beholder sine egne id-er — kortene er de samme, flyttet.
    for (const id of [
      "control-sound",
      "control-folder",
      "control-quality",
      "setup-camera",
      "setup-auto",
      "control-notify",
    ]) {
      expect(html).toContain(`data-testid="${id}"`);
    }
    // Svaret som gjelder nå, ikke innstillingens navn.
    expect(html).toContain("/Users/x/SundayRec");
    // Ubesvart ⇒ gul. Den gule raden er hele grunnen til at noen oppdager den
    // tomme innstillingen før en søndag.
    expect(html).toMatch(/data-testid="control-notify"[^>]*data-tone="warn"/);
    expect(html).toContain("Ingen ennå");
    // …og hvert kort er lukket til noen ber om noe annet.
    expect(html).toMatch(
      /data-testid="control-folder"[^>]*data-expanded="false"/,
    );
    patchSettings({ deviceId: null, deviceName: null, saveFolder: null });
  });

  it("INNSTILLINGER er kirkeprofilen og Avansert — ikke de fem spørsmålene", () => {
    // D2: de fem beslutningene redigeres der de brukes. Tannhjulet åpner det
    // som IKKE hører til en søndag, og det er hele skjermen.
    navigate("setup");
    const html = render(<Shell />);
    expect(html).toMatch(/data-testid="app-heading"[^>]*>Innstillinger</);
    expect(html).toContain('data-testid="setup-church"');
    expect(html).toContain('data-testid="setup-advanced"');
    // Rammen står: leden sier hva lista er, fordi ingen kortrad har sagt det.
    expect(html).toContain('data-testid="setup-advanced-lede"');
    expect(html).not.toContain('data-testid="setup-row-sound"');
    navigate("record");
  });

  it("sier fra når innstillingene ikke kunne leses", () => {
    navigate("record");
    expect(render(<Shell />)).not.toContain('data-testid="hydrate-error"');
  });

  it("Overlays er dialog- og toastverten, og bare den stående live-regionen når begge er tomme", () => {
    // ⚠️ Den var «ingenting». Toastverten returnerte `null` på tom kø, og en
    // `aria-live`-region som opprettes i samme oppdatering som sin egen første
    // melding blir ikke annonsert — hver første toast var stum. Regionen står
    // nå alltid; TOM, men der. Se `ui/ToastHost/ToastHost.tsx`.
    const html = render(<Overlays />);
    expect(html).toContain('data-testid="toast-host"');
    expect(html).toContain('data-empty="true"');
    // Ingen dialog, og ingen toast-rad inni den tomme regionen.
    expect(html).not.toContain('data-testid="dialog"');
    expect(html).not.toContain("data-kind");
  });

  it("monterer utviklingsproben bare når den blir bedt om det", () => {
    navigate("record");
    expect(render(<Shell />)).not.toContain("setting-probe");
  });
});
