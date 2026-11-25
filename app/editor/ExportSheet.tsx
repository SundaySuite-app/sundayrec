/**
 * EKSPORT — arket som åpnes fra «Eksporter» inne i editoren (F5).
 *
 * ## Hvorfor et ark og ikke en side
 *
 * Eksporten var en egen destinasjon (D3) med en tom tilstand som foreslo «sist
 * redigerte opptak». Rent bord: eksport er det siste man gjør med et opptak man
 * allerede har åpent, så den bor INNE i editoren, som et ark over den. Den
 * frittstående siden og rutene dit er borte, og med dem «Idle»-visningen med
 * forslag og filvelger — editoren har alltid en fil.
 *
 * Signalene under (`app/editor/export.ts`, `app/editor/model.ts`) bor på
 * MODULNIVÅ og er prop-frie: en eksport som går overlever at arket lukkes, og
 * kvitteringen står klar neste gang det åpnes.
 *
 * ## Innholdet
 *
 * Tre tilstander, i rekkefølgen de OPPSTÅR i — kvittering vinner over kjøring
 * vinner over valgene: **format**, **innhold** (tittel, taler, beskrivelse),
 * **hvor**, og bryteren «Podkastklar lyd» (samme signal som Lyd-steget). Med
 * video: «Ta med video».
 *
 * ## Det som følger av noe annet, spør vi ikke om
 *
 *   **Bitrate** følger kvalitetsvalget i Innstillinger (`settings.bitrate`).
 *   **Bitdybde** finnes ikke: WAV eksporteres i 16 bit.
 *   **Videokodek** er H.264 — den universelle.
 *
 * ## «Ta med video» finnes bare når det ER video
 *
 * Uten bryteren gir en videofil en ren lydfil. Med bryteren på er containeren
 * mp4, og da er de tre lydformatene ikke et valg: de står dempet med grunnen.
 *
 * ## Originalen røres ikke
 *
 * Bakenden skriver en NY fil med et kollisjonsfritt navn
 * (`<navn>_redigert.<ext>`, `_2` hvis den er opptatt). Linja over knappen er
 * en FORUTSIGELSE av det navnet; kvitteringen viser stien bakenden faktisk
 * svarte med, og den er fasiten.
 *
 * ## Arket er `inert` bak seg
 *
 * Et `role="dialog"` med `aria-modal` i skallets eget tre (ikke
 * `DialogHost`, som er bekreftelseskøen): editoren bak settes `inert` av
 * `EditorPage`, Escape lukker, og fokus flyttes til arket når det åpnes og
 * tilbake til «Eksporter» når det lukkes. Arket kan lukkes midt i en kjøring:
 * eksporten går videre, knappen i editoren sier det, og arket åpnes på
 * fremdriften igjen.
 */

import { signal } from "@preact/signals";
import { useEffect, useRef } from "preact/hooks";

import { exactSpan, keptSeconds } from "./editor-core";
import { profileLabel, resultLine } from "./summary";
import { t, tDyn, tf } from "../i18n";
import { Banner } from "../ui/Banner/Banner";
import { activeDialog } from "../ui/dialog";
import { ActionBar } from "../ui/ActionBar/ActionBar";
import { Button } from "../ui/Button/Button";
import { Card } from "../ui/Card/Card";
import { ProgressBar } from "../ui/ProgressBar/ProgressBar";
import { RadioCards, type RadioOption } from "../ui/RadioCards/RadioCards";
import { revealExport, revealLabel } from "../ui/reveal";
import { TextArea } from "../ui/TextArea/TextArea";
import { TextField } from "../ui/TextField/TextField";
import { Toggle } from "../ui/Toggle/Toggle";
import { podcastAudio, setPodcastAudio } from "./sound";
import {
  cancelExport,
  cancelling,
  exportedBytes,
  exportedFolder,
  exportedLoudness,
  exportedPath,
  exportedRevealToken,
  exportedSeconds,
  exportErrorText,
  exportEtaMs,
  exportFailed,
  exportFormat,
  exportFraction,
  exporting,
  exportPhase,
  exportWasCancelled,
  currentDescription,
  editDescription,
  exportAgain,
  exportFolder,
  exportSpeaker,
  exportTitle,
  includeVideo,
  isVideoExport,
  clearJingle,
  pickExportFolder,
  pickJingle,
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
  localIsoDate,
  megabytes,
  predictedOutputName,
  type ExportFormat,
} from "./export-core";
import { closeFile } from "./loader";
import {
  JINGLE_SLOTS,
  jingleField,
  jingleName,
  type JingleSlot,
} from "./jingles-core";
import {
  cuts,
  duration,
  E,
  fileName,
  loadState,
  mediaInfo,
  startedAtMs,
} from "./model";
import { settings } from "../state/settings";
import { spanLabel } from "./span";
import { DOT } from "@lib/ui/dot";
import styles from "./export-sheet.module.css";

/**
 * Fasen som pågår → teksten som forklarer den. Begge kodene er bakendens, festet mot
 * Rust-siden av `export_phase_codes_match_the_renderer_literals`.
 */
function phaseText(phase: string | null): string {
  return phase === "measuring"
    ? t("editor.exportPhaseMeasuring")
    : t("editor.exportExporting");
}

/** Er arket åpent? Modulsignal, som resten av eksporttilstanden. */
export const exportSheetOpen = signal(false);

export function openExportSheet(): void {
  exportSheetOpen.value = true;
}

export function closeExportSheet(): void {
  exportSheetOpen.value = false;
}

export function ExportSheet() {
  const sheetRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    sheetRef.current?.focus();
    return () => {
      // Etter at gjengivelsen er ferdig: editoren er fortsatt `inert` mens
      // arket rives, og en inert knapp tar ikke imot fokus.
      setTimeout(() => {
        document
          .querySelector<HTMLElement>('[data-testid="editor-export-open"]')
          ?.focus();
      }, 0);
    };
  }, []);

  // Escape på vinduet og ikke på arket: når «Eksporter» (som hadde fokus) byttes
  // ut med fremdriften, står fokus på `<body>`, og en lytter på arket ville
  // aldri sett tasten. En åpen bekreftelsesdialog eier Escape selv.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent): void {
      if (event.key !== "Escape" || activeDialog.peek() !== null) return;
      closeExportSheet();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <div
      class={styles.backdrop}
      data-testid="export-backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) closeExportSheet();
      }}
    >
      <div
        ref={sheetRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="export-sheet-title"
        tabIndex={-1}
        data-testid="export-page"
        data-state={loadState.value}
        class={styles.sheet}
      >
        <div class={styles.sheetHead}>
          <h2 id="export-sheet-title" class={styles.sheetTitle}>
            {t("app.export.title")}
          </h2>
          <Button
            variant="ghost"
            testId="export-close"
            onClick={closeExportSheet}
          >
            {t("app.export.close")}
          </Button>
        </div>
        <Head />
        {exportedPath.value ? (
          <Receipt />
        ) : exporting.value ? (
          <Running />
        ) : (
          <Choices />
        )}
      </div>
    </div>
  );
}

/**
 * Topplinja: hvilken fil, og hva som blir laget av den — to fakta: hvor mye
 * som blir igjen, og hvilken behandling som følger med. Begge kommer fra
 * `app/editor/summary.ts`, som editoren selv leser fra det samme stedet.
 */
function Head() {
  return (
    <p data-testid="export-sub" class={styles.sub}>
      {[fileName.value, resultLine(), profileLabel()].filter(Boolean).join(DOT)}
    </p>
  );
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
  const name = predictedOutputName(
    E.filePath,
    exportExtension(),
    exportTitle.value,
    localIsoDate(startedAtMs.value),
  );
  // Navnet som vises: den valgte mappens (Rust sender bare navnet, aldri
  // stien — A2), eller opptakets egen mappe.
  const picked = exportFolder.value;
  const folderName = picked
    ? picked.displayName
    : folderLabel(folderOf(E.filePath));

  const formats: RadioOption[] = EXPORT_FORMATS.map((id) => ({
    value: id,
    // Formatnavnene er PRODUKTNAVN — «MP3» heter MP3 på begge språk.
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
      description: picked ? picked.displayName : t("app.editor.pickFolderDesc"),
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

      <Content />

      <span class={styles.label}>{t("app.editor.exWhere")}</span>
      <RadioCards
        testId="editor-dest"
        value={picked ? "pick" : "same"}
        options={destinations}
        columns={2}
        onChange={(next) => {
          if (next === "same") exportFolder.value = null;
          else void pickExportFolder();
        }}
      />

      <Card testId="export-podcast-card">
        <div class={styles.autoRow}>
          <div class={styles.autoGrow}>
            <b id="export-podcast-label" class={styles.autoTitle}>
              {t("app.editor.podcastAudio")}
            </b>
            <div id="export-podcast-desc" class={styles.hint}>
              {t("app.editor.podcastAudioDesc")}
            </div>
          </div>
          <Toggle
            testId="export-podcast-toggle"
            checked={podcastAudio.value}
            labelId="export-podcast-label"
            describedBy="export-podcast-desc"
            onChange={setPodcastAudio}
          />
        </div>
      </Card>

      <Jingles video={video} />

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

      <ActionBar
        hint={
          <span data-testid="editor-export-preview">
            {[
              name,
              mb === null ? "" : tf("app.editor.about", { mb }),
              folderName,
            ]
              .filter(Boolean)
              .join(DOT)}
          </span>
        }
      >
        <Button
          variant="primary"
          size="lg"
          testId="editor-export-go"
          onClick={() => void runExport(kept, bytes)}
        >
          {t("editor.save")}
        </Button>
      </ActionBar>
    </div>
  );
}

/**
 * «Intro og outro» — to valgfrie lydfiler som legges før og etter opptaket.
 *
 * Filen velges i et vindu RUST åpner (`settings_pick_editor_intro`/`_outro`),
 * og valget lagres i innstillingene, så det står der neste eksport også. Siden
 * viser bare filnavnet; ingen sti går fra siden til Rust. Med «Podkastklar
 * lyd» på får jinglene samme lydstyrke som talen. Med video er raden dempet
 * og sier hvorfor: motoren legger dem rundt lydsporet, ikke bildet.
 */
function Jingles({ video }: { video: boolean }) {
  return (
    <div data-testid="export-jingles" class={styles.content}>
      <span id="export-jingles-label" class={styles.label}>
        {t("app.editor.jingles")}
      </span>
      <p id="export-jingles-desc" class={styles.hint}>
        {t("app.editor.jinglesDesc")}
      </p>
      <div
        role="group"
        aria-labelledby="export-jingles-label"
        class={styles.content}
      >
        {JINGLE_SLOTS.map((slot) => (
          <JingleRow key={slot} slot={slot} video={video} />
        ))}
      </div>
      {video ? (
        <p data-testid="export-jingles-video-note" class={styles.hint}>
          {t("app.editor.jingleVideoNote")}
        </p>
      ) : null}
    </div>
  );
}

function JingleRow({ slot, video }: { slot: JingleSlot; video: boolean }) {
  const name = jingleName(settings.value[jingleField(slot)]);
  const which =
    slot === "intro"
      ? t("app.editor.jingleIntro")
      : t("app.editor.jingleOutro");
  const reason = video ? t("app.editor.jingleVideoNote") : undefined;
  return (
    <div class={styles.autoRow} data-testid={`export-jingle-${slot}`}>
      <div class={styles.autoGrow}>
        <b class={styles.autoTitle}>{which}</b>
        <div
          class={styles.hint}
          data-testid={`export-jingle-${slot}-name`}
          data-chosen={name ? "true" : "false"}
        >
          {name ?? t("app.editor.jingleNone")}
        </div>
      </div>
      <Button
        variant="secondary"
        testId={`export-jingle-${slot}-pick`}
        disabled={video}
        disabledReason={reason}
        ariaLabel={tf("app.editor.jinglePickLabel", { which })}
        onClick={() => void pickJingle(slot)}
      >
        {t("app.editor.jinglePick")}
      </Button>
      {name ? (
        <Button
          variant="ghost"
          testId={`export-jingle-${slot}-remove`}
          ariaLabel={tf("app.editor.jingleRemoveLabel", { which })}
          onClick={() => void clearJingle(slot)}
        >
          {t("app.editor.jingleRemove")}
        </Button>
      ) : null}
    </div>
  );
}

/**
 * «Innhold» — tittel, taler og beskrivelse. Alt valgfritt.
 *
 * Tittelen blir filnavnet (`export_stem`), og det er navnet et
 * opplastingsskjema foreslår som episodens tittel når fila dras inn. Uten
 * tittel er ingenting endret: fila heter `<navn>_redigert` som før.
 *
 * Feltene er fylt fra opptakets `.meta.json` når de ble skrevet sist
 * (`loadExportContent`). Ingenting lagres før eksporten lykkes.
 */
function Content() {
  return (
    <div data-testid="export-content" class={styles.content}>
      <span class={styles.label}>{t("app.export.content")}</span>
      <div class={styles.field}>
        <span id="export-title-label" class={styles.fieldLabel}>
          {t("app.export.contentTitle")}
        </span>
        <TextField
          value={exportTitle.value}
          onInput={(next) => (exportTitle.value = next)}
          placeholder={t("app.export.contentTitlePlaceholder")}
          labelId="export-title-label"
          describedBy="export-content-hint"
          testId="export-title"
        />
      </div>
      <div class={styles.field}>
        <span id="export-speaker-label" class={styles.fieldLabel}>
          {t("app.export.contentSpeaker")}
        </span>
        <TextField
          value={exportSpeaker.value}
          onInput={(next) => (exportSpeaker.value = next)}
          placeholder={t("app.export.contentSpeakerPlaceholder")}
          labelId="export-speaker-label"
          testId="export-speaker"
        />
      </div>
      <div class={styles.field}>
        <span id="export-description-label" class={styles.fieldLabel}>
          {t("app.export.contentDescription")}
        </span>
        <TextArea
          value={currentDescription()}
          onInput={editDescription}
          placeholder={t("app.export.contentDescriptionPlaceholder")}
          labelId="export-description-label"
          testId="export-description"
        />
      </div>
      <p id="export-content-hint" class={styles.hint}>
        {t("app.export.contentHint")}
      </p>
    </div>
  );
}

/**
 * Hva som gikk galt, når noe gjorde det.
 *
 * En AVBRUTT eksport er ikke en feil — brukeren ba om det — så den er nøytral
 * og ikke rød. De kjente kodene er bakendens egne, og setningene er legacys
 * (og finnes derfor i begge språk).
 *
 * ⚠️ Vakten leser `exportFailed`, IKKE `exportErrorText`: en kode
 * `exportErrorKey` ikke kjenner (en USB-pinne trukket ut, full disk under et
 * annet navn, ffmpeg som feiler av en grunn appen ikke har en setning for) gir
 * `suffix = null` — og hvis vakten så på DEN, viste flaten ingenting i det
 * hele tatt: baren forsvant, og skjemaet sto der som om ingenting skjedde.
 * `exportFailed` skiller «gikk det dårlig» fra «har vi en presis setning for
 * det», så den generelle setningen under alltid er nåbar.
 */
function ExportProblem() {
  const suffix = exportErrorText.value;
  const cancelled = exportWasCancelled.value;
  const failed = exportFailed.value;
  if (!cancelled && !failed) return null;
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
/**
 * «Lydstyrke: passe for nett» — én linje, bare når det ER noe å si (F2-C-B).
 *
 * Bakenden måler det behandlede signalet i Rust og leverer det med ÉN
 * forsterkning (og en true-peak-begrenser bare der toppene trenger det).
 * LUFS-tallet er ikke lenger i teksten: en frivillig kan ikke gjøre noe med
 * «−16,0», men kan forstå «passe for nett». Hvis toppene ikke gir rom for
 * presetets mål, sier linja det i ord. Uten en mastring (eller for et opptak
 * uten nivå å sette) står linja ikke der: en tom påstand er verre enn ingen.
 */
function levelLine(): string {
  const l = exportedLoudness.value;
  if (!l) return "";
  return l.peakLimited ? t("app.editor.levelCapped") : t("app.editor.level");
}

function Receipt() {
  const path = exportedPath.value ?? "";
  const name = path.split(/[/\\]/).pop() ?? path;
  const mb = megabytes(exportedBytes.value);
  const meta = [
    spanLabel(exactSpan(exportedSeconds.value)),
    mb === null ? "" : tf("app.editor.about", { mb }),
    levelLine(),
    folderLabel(exportedFolder.value),
  ]
    .filter(Boolean)
    .join(DOT);

  return (
    <ReceiptCard
      revealToken={exportedRevealToken.value}
      name={name}
      meta={meta}
    />
  );
}

function ReceiptCard({
  revealToken,
  name,
  meta,
}: {
  /** Lappen eksportens svar bar (`recordings_reveal_export`); `null` = fila
   *  fikk ingen, og det er ikke noe å vise. */
  revealToken: string | null;
  name: string;
  meta: string;
}) {
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
          disabled={!revealToken}
          disabledReason={t("app.done.revealFailed")}
          onClick={() => void revealExport(revealToken)}
        >
          {revealLabel()}
        </Button>
        <Button
          variant="secondary"
          testId="editor-exported-again"
          onClick={exportAgain}
        >
          {t("app.editor.exportAgain")}
        </Button>
        {/* «Til opptakene» lukker fila; lista står på samme side. */}
        <Button
          variant="ghost"
          testId="editor-exported-library"
          onClick={() => {
            // Navigeringen henger på svaret: `closeFile` kan si nei (en eksport
            // som pågår, F2-3), og da skal vi bli stående der vi er.
            void closeFile().then((closed) => {
              if (closed) closeExportSheet();
            });
          }}
        >
          {t("app.editor.toLibrary")}
        </Button>
      </div>
    </Card>
  );
}
