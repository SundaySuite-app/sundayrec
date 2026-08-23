/**
 * REDIGER, steg 3 — EKSPORTER. Canvasens artboards 4.3 og 4.4.
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

import { t, tDyn, tf } from "../i18n";
import { navigate } from "../router/router";
import { Banner } from "../ui/Banner/Banner";
import { Button } from "../ui/Button/Button";
import { Card } from "../ui/Card/Card";
import { ProgressBar } from "../ui/ProgressBar/ProgressBar";
import { RadioCards, type RadioOption } from "../ui/RadioCards/RadioCards";
import { Toggle } from "../ui/Toggle/Toggle";
import { exactSpan, keptSeconds } from "./editor-core";
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
} from "./export";
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
} from "./export-core";
import { closeFile } from "./loader";
import { cuts, duration, E, mediaInfo } from "./model";
import { settings } from "../state/settings";
import { soundProfile } from "./sound";
import { spanLabel } from "./span";
import styles from "./editor.module.css";

/** Ett tegn mellom to fakta på samme linje. Ikke prosa. */
const DOT = " · ";

/** Fasen bakenden melder → teksten som forklarer den. To koder, festet mot
 *  Rust-siden av `export_phase_codes_match_the_renderer_literals`. */
function phaseText(phase: string | null): string {
  return phase === "measuring"
    ? t("editor.exportPhaseMeasuring")
    : t("editor.exportExporting");
}

export function ExportStep() {
  if (exportedPath.value) return <Receipt />;
  if (exporting.value) return <Running />;
  return <Choices />;
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
          onClick={() => void window.api.revealFile(path)}
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
        <Button
          variant="ghost"
          testId="editor-exported-library"
          onClick={() => {
            closeFile();
            navigate("library");
          }}
        >
          {t("app.editor.toLibrary")}
        </Button>
      </div>
    </Card>
  );
}

/** Profilens navn, for oppsummeringslinja i toppen av steget. */
export function profileName(): string {
  return tDyn("app.editor.profile", soundProfile.value);
}
