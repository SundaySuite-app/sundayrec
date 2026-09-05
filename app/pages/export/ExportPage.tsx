/**
 * EKSPORTERING — jobb nr. 4 av fire. Canvasens artboards 4.3 og 4.4.
 *
 * ## Hvorfor dette er en DESTINASJON og ikke et steg lenger
 *
 * Fram til D3 var eksporten steg 3 i Rediger. Eieren ba om tre flater i
 * DaVinci-rekkefølge — Opptak · Redigering · Eksportering — fordi mastering og
 * miksing skal kunne bo her på sikt, og et steg inne i et annet steg er ikke et
 * sted noe kan vokse.
 *
 * Fila følger med av seg selv: signalene under (`app/editor/export.ts`,
 * `app/editor/model.ts`) bor på MODULNIVÅ og er prop-frie, så en eksport som
 * går overlever at man ser på biblioteket i mellomtiden. Det er også den ene
 * tingen som må være sann for at den nye plasseringen skal være riktig, og
 * `e2e/export-page.spec.ts` beviser det utenfra.
 *
 * ## Uten en åpen fil er dette ikke en tom side
 *
 * Eiervalg 3 i D3: eksportering skal ALLTID være ett klikk unna. Så `idle`
 * viser det sist redigerte opptaket (`lastEdited`, øktvarig, skrevet av
 * lasteren når en åpning når `ready`) med én primærknapp — «Gjør klar» åpner
 * fila, siden blir stående, og lastetilstanden vises her.
 *
 * Er det ingenting sist redigert, faller kortet tilbake på det SISTE OPPTAKET
 * og sier at det er dét det er. Det er ikke det samme svaret på det samme
 * spørsmålet: `recordings_list` bærer ingen redigert-status
 * (`app/state/recordings.ts`), og et «sist redigert» som egentlig var «sist
 * tatt opp» ville vært appen som gjetter og later som den vet.
 *
 * Atlasets §3d er de ti radene dette erstatter: eksporttype, videoformat,
 * videokodek, format, bitrate, «Bithybde» *(skrivefeil for «Bitdybde», sendt i
 * alle sju språk)*, destinasjon, behandling, intro & outro, lydforbedring —
 * ni klikk hvis alt velges. Her er det to spørsmål: **hvilket format**, og
 * **hvor**.
 *
 * ## Det som følger av noe annet, spør vi ikke om
 *
 *   **Bitrate** følger kvalitetsvalget i Oppsett (`settings.bitrate`, som
 *   `QualityPage` skriver: 256 for «God»). Den som har svart på «hvilken
 *   kvalitet?» én gang har svart.
 *   **Bitdybde** finnes ikke: WAV eksporteres i 16 bit, og de som trenger 24
 *   trenger ikke denne appen.
 *   **Videokodek** er H.264 — den universelle. H.265 halverer fila og kan ikke
 *   spilles av alle, og det er ikke en avveining en frivillig skal ta på vei
 *   ut av en gudstjeneste.
 *   **Behandlingen** står i steg 2 og gjentas ikke her. Legacys «Volum styres
 *   av mastring»-linje fantes fordi normaliseringen og mastringen kunne love
 *   hver sin ting; her er det bare én.
 *
 * ## «Ta med video» finnes bare når det ER video
 *
 * `editor_load_recording` sier `hasVideo` allerede — samme probing som
 * varigheten. En videofil man eksporterer UTEN bryteren gir en ren lydfil, som
 * er hva de fleste vil ha med en gudstjeneste-mp4. Med bryteren på er
 * containeren mp4, og da er de tre lydformatene ikke et valg lenger: de står
 * dempet med grunnen, i stedet for å forsvinne under fingeren.
 *
 * ## Originalen røres ikke
 *
 * Bakenden skriver en NY fil med et kollisjonsfritt navn
 * (`<navn>_redigert.<ext>`, `_2` hvis den er opptatt). Linja over knappen er
 * en FORUTSIGELSE av det navnet; kvitteringen viser stien bakenden faktisk
 * svarte med, og den er fasiten.
 */

import { useEffect } from "preact/hooks";

import { exactSpan, keptSeconds } from "../../editor/editor-core";
import { Loading, LoadFailed } from "../../editor/LoadStates";
import { profileLabel, resultLine } from "../../editor/summary";
import { locale, t, tDyn, tf } from "../../i18n";
import { navigate } from "../../router/router";
import {
  lastRecording,
  loadRecordingCount,
  recordings,
} from "../../state/recordings";
import { Banner } from "../../ui/Banner/Banner";
import { Button } from "../../ui/Button/Button";
import { Card } from "../../ui/Card/Card";
import { EmptyState } from "../../ui/EmptyState/EmptyState";
import { ProgressBar } from "../../ui/ProgressBar/ProgressBar";
import { RadioCards, type RadioOption } from "../../ui/RadioCards/RadioCards";
import { reveal } from "../../ui/reveal";
import { Toggle } from "../../ui/Toggle/Toggle";
import {
  cancelExport,
  cancelling,
  exportedBytes,
  exportedFolder,
  exportedPath,
  exportedSeconds,
  exportErrorText,
  exportEtaMs,
  exportFormat,
  exportFraction,
  exporting,
  exportPhase,
  exportWasCancelled,
  exportAgain,
  exportFolder,
  includeVideo,
  isVideoExport,
  pickExportFolder,
  runExport,
  sourceHasVideo,
  exportExtension,
} from "../../editor/export";
import {
  bitrateKbps,
  estimatedBytes,
  EXPORT_FORMATS,
  exportKbps,
  folderLabel,
  folderOf,
  megabytes,
  predictedOutputName,
  type ExportFormat,
} from "../../editor/export-core";
import { closeFile, openFile, pickAndOpen } from "../../editor/loader";
import {
  cuts,
  duration,
  E,
  fileName,
  lastEdited,
  loadState,
  mediaInfo,
} from "../../editor/model";
import { settings } from "../../state/settings";
import { spanLabel } from "../../editor/span";
import { dateTimeTitle } from "@lib/ui/date-title";
import { DOT } from "@lib/ui/dot";
import styles from "./export.module.css";

/** Fasen bakenden melder → teksten som forklarer den. To koder, festet mot
 *  Rust-siden av `export_phase_codes_match_the_renderer_literals`. */
function phaseText(phase: string | null): string {
  return phase === "measuring"
    ? t("editor.exportPhaseMeasuring")
    : t("editor.exportExporting");
}

export function ExportPage() {
  const state = loadState.value;

  return (
    <div data-testid="export-page" data-state={state} class={styles.page}>
      {state === "ready" ? (
        <Ready />
      ) : state === "loading" ? (
        <Loading />
      ) : state === "error" ? (
        <LoadFailed />
      ) : (
        <Idle />
      )}
    </div>
  );
}

/**
 * Med en fil åpen: topplinja, og den ene av tre tilstander eksporten er i.
 *
 * Rekkefølgen er ikke tilfeldig. En kvittering vinner over en kjøring vinner
 * over valgene, fordi det er den rekkefølgen de OPPSTÅR i — og fordi et
 * sidebytte midt i en eksport skal komme tilbake til det som gjelder, ikke til
 * skjemaet man fylte ut for to minutter siden.
 */
function Ready() {
  return (
    <>
      <Head />
      {exportedPath.value ? (
        <Receipt />
      ) : exporting.value ? (
        <Running />
      ) : (
        <Choices />
      )}
    </>
  );
}

/**
 * Topplinja: hvilken fil, og hva som blir laget av den.
 *
 * Slank med vilje. REDIGERING har filnavn og varighet i sin egen topplinje;
 * her er spørsmålet «hva er det egentlig jeg eksporterer», og svaret på det er
 * to fakta: hvor mye som blir igjen, og hvilken behandling som følger med.
 * Begge kommer fra `app/editor/summary.ts`, som steg 2 leser fra det samme
 * stedet.
 */
function Head() {
  return (
    <p data-testid="export-sub" class={styles.sub}>
      {[fileName.value, resultLine(), profileLabel()].filter(Boolean).join(DOT)}
    </p>
  );
}

// ── Uten en åpen fil ────────────────────────────────────────────────────────

/**
 * «Sist redigert», en velger, og «Åpne fil…».
 *
 * Eiervalg 3 i D3: fra denne siden skal en eksport ALLTID være ett klikk unna.
 * Tre veier, i den rekkefølgen de er sannsynlige — den ene man nettopp
 * redigerte, en av de andre man har tatt opp, eller en fil fra et annet sted.
 *
 * ⚠️ Ingen av kortene påstår at noe ER redigert. `recordings_list` bærer ingen
 * slik status, og sidevognen ved siden av opptaket ville vært en gjetning: et
 * kutt-utkast betyr at noen begynte, ikke at noe ble gjort. Så kortet sier
 * enten «Sist redigert» (fordi vi SÅ det skje i denne økta) eller «Siste
 * opptak» (fordi det er alt vi vet).
 */
function Idle() {
  const edited = lastEdited.value;
  const last = lastRecording.value;
  const rows = recordings.value;

  // Lista leses når siden åpnes: et opptak som ble tatt mens man sto på OPPTAK
  // skal være her når man går hit.
  useEffect(() => {
    void loadRecordingCount();
  }, []);

  const suggestion = edited
    ? {
        path: edited.path,
        name: edited.fileName,
        atMs: edited.startedAtMs,
        label: t("app.export.lastEdited"),
      }
    : last?.path
      ? {
          path: last.path,
          name: last.filename,
          atMs: last.startedAt ?? last.timestamp ?? null,
          label: t("app.record.last"),
        }
      : null;

  // Alt annet enn den ene som allerede står øverst. En liste som tilbyr det
  // kortet over den tilbyr er to knapper for den samme handlingen.
  const others = (rows ?? []).filter(
    (row) => row.path && row.path !== suggestion?.path,
  );

  return (
    <div class={styles.idle}>
      {suggestion ? (
        <div class={styles.recent}>
          <span class={styles.label}>{suggestion.label}</span>
          <div data-testid="export-last" class={styles.recentRow}>
            <div class={styles.recentGrow}>
              <div class={styles.value}>
                {suggestion.atMs === null
                  ? suggestion.name
                  : dateTimeTitle(suggestion.atMs, locale.value)}
              </div>
              <div class={styles.recentName}>{suggestion.name}</div>
            </div>
            <Button
              variant="primary"
              testId="export-last-open"
              onClick={() =>
                void openFile(suggestion.path, {
                  startedAtMs: suggestion.atMs,
                })
              }
            >
              {t("app.export.prepare")}
            </Button>
          </div>
        </div>
      ) : null}

      {others.length > 0 ? (
        <div class={styles.recent}>
          <span class={styles.label}>{t("app.export.pickTitle")}</span>
          <div data-testid="export-pick" class={styles.list}>
            {others.map((row) => (
              <div
                key={row.path}
                data-testid="export-pick-row"
                data-path={row.path ?? undefined}
                class={styles.recentRow}
              >
                <div class={styles.recentGrow}>
                  <div class={styles.value}>
                    {rowWhen(row.startedAt ?? row.timestamp ?? null) ||
                      row.filename}
                  </div>
                  <div class={styles.recentName}>{row.filename}</div>
                </div>
                <Button
                  variant="secondary"
                  testId="export-pick-use"
                  onClick={() =>
                    void openFile(row.path as string, {
                      startedAtMs: row.startedAt ?? row.timestamp ?? null,
                    })
                  }
                >
                  {t("app.export.use")}
                </Button>
              </div>
            ))}
          </div>
        </div>
      ) : null}

      {/* Ikke lest ennå ⇒ ingen påstand i noen retning. Er det FAKTISK
          ingenting, sier siden det — og tilbyr den ene veien som finnes
          uansett: en fil fra et annet sted. */}
      {suggestion === null && rows !== null && others.length === 0 ? (
        <EmptyState
          testId="export-empty"
          title={t("app.page.export")}
          description={t("app.export.emptyDesc")}
          action={
            <Button
              variant="primary"
              testId="export-open"
              onClick={() => void pickAndOpen()}
            >
              {t("editor.openFile")}
            </Button>
          }
        />
      ) : (
        <div class={styles.toolbar}>
          <Button
            variant="secondary"
            testId="export-open"
            onClick={() => void pickAndOpen()}
          >
            {t("editor.openFile")}
          </Button>
        </div>
      )}
    </div>
  );
}

/** Datoen for en rad, eller tom streng når raden ikke har en. Samme tittel
 *  («Søndag 2. august 2026 · 11:00») raden har i Bibliotek, fordi det er den
 *  frivillige kjenner opptaket sitt på — se `@lib/ui/date-title`. */
function rowWhen(atMs: number | null): string {
  return atMs === null ? "" : dateTimeTitle(atMs, locale.value);
}

// ── Valgene ─────────────────────────────────────────────────────────────────

function Choices() {
  const video = isVideoExport();
  const kept = keptSeconds(cuts.value, duration.value);
  const info = mediaInfo.value;

  const kbps = exportKbps(
    exportFormat.value,
    { channels: info?.channels ?? null, sampleRate: info?.sampleRate ?? null },
    bitrateKbps(settings.value.bitrate),
  );
  // Ingen anslag for video: en omkoding av bildet har en bitrate vi ikke kan
  // regne ut på forhånd, og et tall vi ikke kan regne ut skal vi ikke vise.
  const bytes = video ? null : estimatedBytes(kept, kbps);
  const mb = megabytes(bytes);
  const name = predictedOutputName(E.filePath, exportExtension());
  const folder = exportFolder.value || folderOf(E.filePath);

  const formats: RadioOption[] = EXPORT_FORMATS.map((id) => ({
    value: id,
    // Formatnavnene er PRODUKTNAVN — «MP3» heter MP3 på alle sju språk.
    title: id.toUpperCase(),
    description: tDyn("app.editor.fmtDesc", id),
    recommended: id === "mp3",
  }));

  const destinations: RadioOption[] = [
    {
      value: "same",
      title: t("app.editor.sameFolder"),
      description: folderLabel(folderOf(E.filePath)),
    },
    {
      value: "pick",
      title: t("app.editor.pickFolder"),
      description: exportFolder.value
        ? folderLabel(exportFolder.value)
        : t("app.editor.pickFolderDesc"),
    },
  ];

  return (
    <div data-testid="editor-export" class={styles.step}>
      <ExportProblem />

      <span class={styles.label}>{t("app.editor.exFormat")}</span>
      <RadioCards
        testId="editor-format"
        value={exportFormat.value}
        options={formats}
        disabled={video}
        onChange={(next) => (exportFormat.value = next as ExportFormat)}
      />
      {video ? (
        <p data-testid="editor-format-locked" class={styles.hint}>
          {t("app.editor.videoLocksFormat")}
        </p>
      ) : null}

      <span class={styles.label}>{t("app.editor.exWhere")}</span>
      <RadioCards
        testId="editor-dest"
        value={exportFolder.value ? "pick" : "same"}
        options={destinations}
        columns={2}
        onChange={(next) => {
          if (next === "same") exportFolder.value = "";
          else void pickExportFolder();
        }}
      />

      {sourceHasVideo() ? (
        <Card testId="editor-video-card">
          <div class={styles.autoRow}>
            <div class={styles.autoGrow}>
              <b id="editor-video-label" class={styles.autoTitle}>
                {t("app.editor.includeVideo")}
              </b>
              <div id="editor-video-desc" class={styles.hint}>
                {t("app.editor.includeVideoDesc")}
              </div>
            </div>
            <Toggle
              testId="editor-video-toggle"
              checked={includeVideo.value}
              labelId="editor-video-label"
              describedBy="editor-video-desc"
              onChange={(next) => (includeVideo.value = next)}
            />
          </div>
        </Card>
      ) : null}

      <div class={styles.exportBar}>
        <span data-testid="editor-export-preview" class={styles.hint}>
          {[
            name,
            mb === null ? "" : tf("app.editor.about", { mb }),
            folderLabel(folder),
          ]
            .filter(Boolean)
            .join(DOT)}
        </span>
        <Button
          variant="primary"
          size="lg"
          testId="editor-export-go"
          onClick={() => void runExport(kept, bytes)}
        >
          {t("editor.save")}
        </Button>
      </div>
    </div>
  );
}

/**
 * Hva som gikk galt, når noe gjorde det.
 *
 * En AVBRUTT eksport er ikke en feil — brukeren ba om det — så den er nøytral
 * og ikke rød. De fem andre kodene er bakendens egne, og setningene er legacys
 * (og finnes derfor i alle sju språk). En kode vi ikke kjenner får den
 * generelle setningen i stedet for en råstreng fra en annen prosess.
 */
function ExportProblem() {
  const suffix = exportErrorText.value;
  const cancelled = exportWasCancelled.value;
  if (!suffix && !cancelled) return null;
  return (
    <Banner
      tone={cancelled ? "warn" : "bad"}
      testId="editor-export-error"
      // `tDyn("editor", …)` av samme grunn som lastefasene i P4a: prefikset er
      // en literal gaten kan slå opp, suffikset er bakendens kode.
      title={suffix ? tDyn("editor", suffix) : t("app.editor.exportFailed")}
    />
  );
}

// ── Fremdriften ─────────────────────────────────────────────────────────────

function Running() {
  const fraction = exportFraction.value;
  const label = phaseText(exportPhase.value);

  return (
    <div data-testid="editor-exporting" class={styles.step}>
      <ProgressBar
        // Ubestemt til bakenden har et ekte tall: mastringens måle-passering
        // melder 0 %, og en bar som står på null i to minutter leses som hengt.
        fraction={fraction ?? 0}
        etaMs={exportEtaMs.value}
        label={label}
        hideReadout={fraction === null}
        testId="editor-export-progress"
      />
      <div class={styles.toolbar}>
        <Button
          variant="secondary"
          testId="editor-export-cancel"
          onClick={() => void cancelExport()}
        >
          {cancelling.value
            ? t("editor.exportCancelling")
            : t("editor.exportCancel")}
        </Button>
      </div>
    </div>
  );
}

// ── Kvitteringen ────────────────────────────────────────────────────────────

/**
 * «Eksportert» — den samme kvitteringsformen som etter et opptak.
 *
 * Filnavnet er bakendens, ikke vår forutsigelse: den er den ENE som vet om det
 * lå en fil med det navnet der fra før.
 */
function Receipt() {
  const path = exportedPath.value ?? "";
  const name = path.split(/[/\\]/).pop() ?? path;
  const mb = megabytes(exportedBytes.value);
  const meta = [
    spanLabel(exactSpan(exportedSeconds.value)),
    mb === null ? "" : tf("app.editor.about", { mb }),
    folderLabel(exportedFolder.value),
  ]
    .filter(Boolean)
    .join(DOT);

  return (
    <Card
      tone="good"
      testId="editor-exported"
      title={t("app.editor.exported")}
      description={meta}
    >
      <div data-testid="editor-exported-file" class={styles.value}>
        {name}
      </div>
      <div class={styles.toolbar}>
        <Button
          variant="primary"
          testId="editor-exported-reveal"
          onClick={() => void reveal(path)}
        >
          {t("app.done.show")}
        </Button>
        <Button
          variant="secondary"
          testId="editor-exported-again"
          onClick={exportAgain}
        >
          {t("app.editor.exportAgain")}
        </Button>
        {/*
          Herfra er «Til biblioteket» en EKTE navigering: eksporteringen er en
          annen destinasjon enn redigeringen etter D3, så å lukke fila uten å
          flytte seg ville latt brukeren stå igjen på en side som nettopp
          mistet det den handlet om.
        */}
        <Button
          variant="ghost"
          testId="editor-exported-library"
          onClick={() => {
            closeFile();
            navigate("edit");
          }}
        >
          {t("app.editor.toLibrary")}
        </Button>
      </div>
    </Card>
  );
}
