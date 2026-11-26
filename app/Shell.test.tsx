import { render } from "preact-render-to-string";
import { describe, expect, it } from "vitest";

import type { RecordingEntry } from "@legacy/types";

import { Overlays, Shell } from "./Shell";
import { loadState } from "./editor/model";
import { navigate, route } from "./router/router";
import { setLocale } from "./i18n";
import { globalError } from "./state/global-error";
import { isRecording } from "./state/recording";
import { recordings } from "./state/recordings";
import { trashEntries } from "./state/trash";
import { hydrateError, patchSettings } from "./state/settings";

// Fortsatt det ene stedet i enhetsgaten som beviser (a) at `.tsx` kompilerer
// uten et Babel-forvalg — transformen er tsconfigs `jsxImportSource: "preact"`
// — og (b) at `@lib/*` når fram til legacy-rendereren, så det nye skallet
// leser de SAMME to katalogene den utsendte appen gjør.
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
    expect(html).toContain("Ta opp");
    expect(html).toContain('data-testid="app-heading"');
    expect(html).toContain('data-testid="nav-record"');
    expect(html).toContain("data-tauri-drag-region");
  });

  it("skjuler fanene og tannhjulet under opptak og viser «Tar opp»", () => {
    navigate("record");
    isRecording.value = true;
    try {
      const html = render(<Shell />);
      expect(html).toContain('data-testid="topbar-recording"');
      expect(html).not.toContain('data-testid="nav-record"');
      expect(html).not.toContain('data-testid="nav-recordings"');
      expect(html).not.toContain('data-testid="nav-settings"');
    } finally {
      isRecording.value = false;
    }
  });

  it("er tre bånd i rekkefølge: topplinje, side, bunnlinje — og skinnen er borte", () => {
    // D3: venstreskinnen er revet. Det som var i den bor nå i to linjer, og
    // REKKEFØLGEN i treet er den samme som rekkefølgen på skjermen — det er den
    // som bestemmer hva en skjermleser og en tabtast møter først.
    navigate("record");
    const html = render(<Shell />);

    const top = html.indexOf('data-testid="topbar"');
    const main = html.indexOf('data-testid="main"');
    const bottom = html.indexOf('data-testid="bottombar"');
    expect(top).toBeGreaterThan(-1);
    expect(main).toBeGreaterThan(top);
    expect(bottom).toBeGreaterThan(main);
    // Skinnen finnes ikke lenger — verken som element eller som testid.
    expect(html).not.toContain('data-testid="rail"');

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

    // Topplinja (E1): merket, to ikonfaner og tannhjulet. Ingen Plan-fane.
    const topbar = html.slice(top, main);
    expect(topbar).toContain('data-testid="app-logo"');
    expect(html).not.toContain('data-testid="shell-church"');
    for (const id of ["nav-record", "nav-recordings", "nav-settings"]) {
      expect(topbar).toContain(`data-testid="${id}"`);
    }
    expect(html).not.toContain('data-testid="nav-plan"');

    // Bunnlinja: statuslinjen.
    const bar = html.slice(bottom);
    for (const id of ["status-line", "status-dot", "status-text"]) {
      expect(bar).toContain(`data-testid="${id}"`);
    }
  });

  it("følger ruten, og språket", async () => {
    // D2: destinasjonen heter «Innstillinger» og bor på tannhjulet nederst i
    // skinnen. Ruten er den samme (`setup`); det er navnet og plasseringen som
    // flyttet, og teksten kommer fortsatt fra katalogen.
    navigate("settings");
    expect(render(<Shell />)).toContain("Innstillinger");
    await setLocale("en");
    // Sluttet `@lib` å resolve, ville `t()` gitt tom tekst og dette vært en
    // tom `<h1>` — som er nøyaktig hvordan en stille ødelagt alias ser ut.
    expect(render(<Shell />)).toContain("Settings");
    await setLocale("no");
  });

  it("bærer den gamle logoen, og Innstillinger som fjerde side", () => {
    navigate("record");
    const html = render(<Shell />);

    // Merket fra den utsendte appen, ikke den gule «S»-boksen.
    expect(html).toContain('data-testid="app-logo"');
    // ⚠️ `<defs>`-id-er er GLOBALE i dokumentet. Prefikset er kollisjonsvakten
    // mot `src-tauri/app-icon.svg`s generiske `bg`/`glow`/`gold`/`clip` — se
    // filhodet i `ui/AppLogo/AppLogo.tsx`.
    // E1: toppfeltet bruker `bare`-varianten (kors + buer, ingen flis/klipp).
    expect(html).not.toContain('id="srlogo-clip"');
    expect(html).toContain('id="srlogo-gold"');
    expect(html).toContain('viewBox="152 163 720 720"');
    expect(html).not.toContain('id="clip"');
    expect(html).not.toContain('id="gold"');

    // `nav-settings` sier fra når man står der.
    expect(html).toContain('data-testid="nav-settings"');
    expect(html).not.toMatch(/data-testid="nav-settings"[^>]*aria-current/);
    navigate("settings");
    expect(render(<Shell />)).toMatch(
      /data-testid="nav-settings"[^>]*aria-current="page"/,
    );
    navigate("record");
  });

  it("bærer ruten som attributter, ikke som synlig feilsøkingstekst", () => {
    // På INNSTILLINGER er en `?goto=`-fane et anker, og bare det.
    navigate("settings", { tab: "settings-camera" });
    const html = render(<Shell />);
    expect(html).toContain('data-page="settings"');
    expect(html).toContain('data-anchor="camera"');
    expect(html).not.toContain("data-tab=");
    expect(route.value).toEqual({
      page: "settings",
      anchor: "camera",
      highlight: true,
    });

    // OPPTAK bærer papirkurven som en fane — og ingen fane uten den.
    navigate("recordings");
    const mainTag = (markup: string): string => {
      const at = markup.indexOf('data-testid="main"');
      return markup.slice(markup.lastIndexOf("<", at), markup.indexOf(">", at));
    };
    const list = mainTag(render(<Shell />));
    expect(list).toContain('data-page="recordings"');
    expect(list).not.toContain("data-tab=");
    navigate("recordings", { tab: "trash" });
    expect(mainTag(render(<Shell />))).toContain('data-tab="trash"');
    navigate("record");
  });

  it("TA OPP sier fra når ingen lydkilde er valgt — og peker på INNSTILLINGER", () => {
    navigate("record");
    patchSettings({ deviceName: null, deviceId: null });
    const html = render(<Shell />);
    expect(html).toContain('data-testid="record-no-source"');
    expect(html).toContain('data-testid="record-choose-sound"');
  });

  it("TA OPP viser kilden og måleren når kilden ER valgt", () => {
    navigate("record");
    patchSettings({ deviceName: "Behringer X32", deviceId: "x32" });
    const html = render(<Shell />);
    expect(html).toContain('data-testid="record-change-source"');
    expect(html).toContain('data-testid="record-vu"');
    expect(html).toContain("Behringer X32");
    expect(html).not.toContain('data-testid="record-no-source"');
    patchSettings({ deviceName: null, deviceId: null });
  });

  it("TA OPP sperrer Start med en GRUNN, ikke med en grå knapp", () => {
    navigate("record");
    patchSettings({ deviceName: null, deviceId: null });
    const html = render(<Shell />);
    // `aria-disabled`, aldri `disabled`: en tastaturbruker skal kunne komme
    // fram til knappen for å HØRE hvorfor den er av.
    expect(html).toMatch(/data-testid="record-start"[^>]*aria-disabled="true"/);
    expect(html).not.toMatch(/data-testid="record-start"[^>]* disabled/);
    expect(html).toContain("Start er sperret til dere har valgt lydkilde");
  });

  it("OPPTAK påstår ingenting før opptakene er talt", () => {
    navigate("recordings");
    recordings.value = null;
    const html = render(<Shell />);
    // Verken «ingen opptak» eller en liste: vi vet ikke ennå. Papirkurv-ikonet
    // står likevel — den er den ene tingen som ALLTID skal ha en dør.
    expect(html).not.toContain('data-testid="library-empty"');
    expect(html).not.toContain('data-testid="library-row"');
    expect(html).toContain('data-testid="library-trash-open"');
  });

  it("OPPTAK viser tomtilstanden bare når det FAKTISK er tomt", () => {
    navigate("recordings");
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

  it("OPPTAK skjuler en rad som ligger i papirkurven, og viser den igjen når angre fjerner den fra signalet (F2-A-F)", () => {
    // Funnet: `toLibraryRows`/`LibraryPage.tsx` filtrerte ikke selv — bare
    // `getHistory()` gjorde det, ved LASTING. Denne testen beviser at lista
    // også følger `trashEntries`-SIGNALET uten en ny historikklasting: det er
    // nøyaktig det «Angre» (som skriver til dette signalet før
    // `loadRecordingCount()` er ferdig) er avhengig av for å ikke la den
    // gjenopprettede raden mangle en runde til.
    navigate("recordings");
    recordings.value = [
      row({ path: "/a.mp3", filename: "a.mp3" }),
      row({ path: "/b.mp3", filename: "b.mp3" }),
    ];
    trashEntries.value = [
      {
        id: "t1",
        originalPath: "/a.mp3",
        trashedPath: "/trash/1-a.mp3",
        name: "a.mp3",
        deletedAt: Date.now(),
        related: [],
        byteSize: 1000,
        recordingId: null,
      },
    ];
    const trashed = render(<Shell />);
    expect(trashed).not.toContain("a.mp3");
    expect(trashed).toContain("b.mp3");
    expect(trashed).toContain("Opptak: 1");

    // En sti papirkurven ikke nevner skal ikke røre noe — ukjent sti, uendret.
    trashEntries.value = [
      {
        id: "t2",
        originalPath: "/ukjent.mp3",
        trashedPath: "/trash/2-ukjent.mp3",
        name: "ukjent.mp3",
        deletedAt: Date.now(),
        related: [],
        byteSize: 1000,
        recordingId: null,
      },
    ];
    const unrelated = render(<Shell />);
    expect(unrelated).toContain("a.mp3");
    expect(unrelated).toContain("b.mp3");
    expect(unrelated).toContain("Opptak: 2");

    // Angre: papirkurven mister raden — samme signal, ingen ny lasting.
    trashEntries.value = [];
    const restored = render(<Shell />);
    expect(restored).toContain("a.mp3");
    expect(restored).toContain("Opptak: 2");

    trashEntries.value = null;
    recordings.value = null;
  });

  it("OPPTAK har et papirkurv-ikon også når kurven er tom", () => {
    // Atlaset §5, funn 9: legacy skjuler «Papirkurv»-lenken når `trash_list`
    // er tom, så en frivillig som slettet noe i går og leter etter det i dag
    // finner ingen dør hvis sveipen har vært innom i mellomtiden.
    navigate("recordings");
    recordings.value = [];
    trashEntries.value = [];
    const html = render(<Shell />);
    expect(html).toContain('data-testid="library-trash-open"');
    expect(html).toContain('aria-label="Papirkurv"');
    trashEntries.value = null;
    recordings.value = null;
  });

  it("PAPIRKURVEN er en visning inne i OPPTAK, ikke en egen side", () => {
    navigate("recordings", { tab: "trash" });
    const html = render(<Shell />);
    // Siden står på OPPTAK hele veien; ikonet bytter innholdet.
    expect(html).toMatch(
      /data-testid="nav-recordings"[^>]*aria-current="page"/,
    );
    expect(html).toMatch(/data-testid="app-heading"[^>]*>Opptak</);
    expect(html).toContain('data-testid="trash-back"');
    expect(html).toContain('data-testid="trash-lede"');
    // …og papirkurven er UTENFOR slippsonen: et opptak sluppet på den ville
    // sett ut som en handling, og den ene handlingen det ligner på er den vi
    // ikke gjør.
    expect(html).not.toContain('data-testid="edit-dropzone"');
    navigate("recordings");
  });

  it("OPPTAK viser biblioteket til en fil er på gang — og bytter da", () => {
    // Bryteren er `loadState`, ikke `hasFile`: se `app/Shell.tsx`. Grenen er
    // hele grunnen til at biblioteket aldri blinker innom under en åpning, og
    // e2e ser det samme utenfra (`e2e/editor.spec.ts`).
    navigate("recordings");
    recordings.value = [row({ path: "/a.mp3", filename: "a.mp3" })];
    loadState.value = "idle";
    const list = render(<Shell />);
    expect(list).toContain('data-testid="library-row"');
    expect(list).not.toContain('data-testid="editor"');
    // Slippsonen omslutter begge visningene — en fil fra en annen opptaker
    // skal kunne slippes mens LISTA står, som er mesteparten av tiden.
    expect(list).toContain('data-testid="edit-dropzone"');
    // Overskriften er destinasjonens eget navn så lenge ingen fil er åpen.
    expect(list).toMatch(/data-testid="app-heading"[^>]*>Opptak</);

    loadState.value = "loading";
    const editing = render(<Shell />);
    expect(editing).toContain('data-testid="editor"');
    expect(editing).not.toContain('data-testid="library-row"');
    expect(editing).toContain('data-testid="edit-dropzone"');

    loadState.value = "idle";
    recordings.value = null;
  });

  it("TA OPP er beskåret: kilde, måler og Start — uten oppsettskortene", () => {
    navigate("record");
    patchSettings({
      deviceId: "x32",
      deviceName: "Behringer X32",
      saveFolder: "/Users/x/SundayRec",
    });
    const html = render(<Shell />);
    for (const id of ["control-sound", "record-vu", "record-start"]) {
      expect(html).toContain(`data-testid="${id}"`);
    }
    // Kamera, auto, mappe, kvalitet og varsling bor på INNSTILLINGER og PLAN.
    for (const id of [
      "setup-camera",
      "setup-auto",
      "control-folder",
      "control-quality",
      "control-notify",
    ]) {
      expect(html).not.toContain(`data-testid="${id}"`);
    }
    // Linja om neste planlagte opptak har en vei til PLAN.
    expect(html).toContain('data-testid="record-next-plan"');
    patchSettings({ deviceId: null, deviceName: null, saveFolder: null });
  });

  it("`plan` er ikke en side: TA OPP med ukestripa åpen — fliser og ukevisning", () => {
    navigate("plan");
    const html = render(<Shell />);
    expect(html).toMatch(/data-testid="app-heading"[^>]*>Ta opp</);
    expect(html).toContain('data-testid="strip-tiles"');
    expect(html).toContain('data-testid="strip-tile-auto"');
    expect(html).toContain('data-testid="strip-wake-more"');
    expect(html).toContain('data-testid="plan-week"');
    // Den gamle PLAN-siden og skjemaet fra før er borte.
    expect(html).not.toContain('data-testid="plan-page"');
    expect(html).not.toContain('data-testid="adv-slot-add"');
    navigate("record");
  });

  it("INNSTILLINGER har alle seksjonene, med lede", () => {
    navigate("settings");
    const html = render(<Shell />);
    expect(html).toMatch(/data-testid="app-heading"[^>]*>Innstillinger</);
    expect(html).toContain('data-testid="setup-toc"');
    for (const id of ["notify", "language", "updates", "privacy", "about"]) {
      expect(html, id).toContain(`data-testid="setup-sec-${id}"`);
    }
    // Lyd, kamera, mappe og opptaksvalg flyttet til lydarket (E4).
    for (const id of ["sound", "camera", "folder", "recording"]) {
      expect(html, id).not.toContain(`data-testid="setup-sec-${id}"`);
    }
    // Skjermene inni er de samme som første gang, og rammen (leden) står.
    expect(html).toContain('data-testid="setup-language-lede"');
    // Tidsplanen bor på PLAN.
    expect(html).not.toContain('data-testid="advanced-schedule"');
    navigate("record");
  });

  it("sier fra når innstillingene ikke kunne leses", () => {
    navigate("record");
    expect(render(<Shell />)).not.toContain('data-testid="hydrate-error"');
  });

  it("en feilet lesing gir banneret en «Prøv igjen»-knapp", () => {
    navigate("record");
    hydrateError.value = "settingsLoadFailed";
    try {
      const html = render(<Shell />);
      expect(html).toContain('data-testid="hydrate-error"');
      expect(html).toContain('data-testid="hydrate-retry"');
    } finally {
      hydrateError.value = null;
    }
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
    // Og ingen feilbanner uten en feil (se testene under).
    expect(html).not.toContain('data-testid="banner-global-error"');
  });

  it("globalError (R5): Overlays viser banneret med den RÅ meldingen, «Kopier» og et kryss", () => {
    // `installErrorHandlers()` setter signalet fra en uventet `error`/
    // `unhandledrejection` — se `state/global-error.ts`. Detaljen er meldingen
    // SLIK DEN KOM, uoversatt: den er for oss (et GitHub-issue, en e-post til
    // dev@), ikke for brukeren. Bare rammen rundt er oversatt.
    globalError.value = "TypeError: cannot read properties of undefined";
    const html = render(<Overlays />);
    expect(html).toContain('data-tone="bad" data-testid="banner-global-error"');
    expect(html).toContain("Noe gikk galt");
    expect(html).toContain("TypeError: cannot read properties of undefined");
    expect(html).toContain('data-testid="banner-global-error-copy"');
    expect(html).toContain('data-testid="banner-global-error-dismiss"');
    globalError.value = null;
  });

  it("monterer utviklingsproben bare når den blir bedt om det", () => {
    navigate("record");
    expect(render(<Shell />)).not.toContain("setting-probe");
  });
});
