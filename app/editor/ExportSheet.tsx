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
 * ## E7: et sidearks, ikke en modal midt på skjermen
 *
 * Samme ark som lydarket på Ta opp (440 px fra høyre, glid, slør, Esc, fokusfelle
 * — `app/ui/sheet-behavior.ts`). Formatkort (MP3 «Podkast», WAV «Arkiv», MP4
 * «Video» — bare når kilden HAR bilde; MP4 ER «ta med video»), tre fliser
 * (Podkast, Intro, Outro), mappe-raden og en stor «Lagre». FLAC er ikke et valg
 * i arket lenger (motoren kan det fortsatt; ingen av eierens formater er det).
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
import { useEffect, useRef, useState } from "preact/hooks";

import { exactSpan, keptSeconds } from "./editor-core";
import { profileLabel, resultLine } from "./summary";
import { t, tDyn, tf } from "../i18n";
import { Banner } from "../ui/Banner/Banner";
import { activeDialog } from "../ui/dialog";
import { Button } from "../ui/Button/Button";
import { Card } from "../ui/Card/Card";
import { ProgressBar } from "../ui/ProgressBar/ProgressBar";
import { RadioCards, type RadioOption } from "../ui/RadioCards/RadioCards";
import { revealExport, revealLabel } from "../ui/reveal";
import { TextArea } from "../ui/TextArea/TextArea";
import { TextField } from "../ui/TextField/TextField";
import { Icon } from "../ui/Icon/Icon";
import { useSheetBehavior } from "../ui/sheet-behavior";
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
  introOn,
  isVideoExport,
  outroOn,
  jingleOnSignal,
  pickExportFolder,
  pickJingle,
  runExport,
  sourceHasVideo,
  exportExtension,
} from "./export";
import {
  bitrateKbps,
  estimatedBytes,
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

function closeExportSheet(): void {
  exportSheetOpen.value = false;
}

/** Hvor lenge arket står i treet etter at det er lukket: glidet ut. */
const SLIDE_MS = 450;

/**
 * Arket. Alltid montert av arbeidsflaten; innholdet står i treet bare mens
 * arket er åpent (og glir ut), så en lukket editor ikke bærer et helt skjema.
 */
export function ExportSheet({ open }: { open: boolean }) {
  const [mounted, setMounted] = useState(open);
  // Glidet inn trenger et frame med arket utenfor kanten først.
  const [entered, setEntered] = useState(false);
  useEffect(() => {
    if (open) {
      setMounted(true);
      const id = requestAnimationFrame(() =>
        requestAnimationFrame(() => setEntered(true)),
      );
      return () => cancelAnimationFrame(id);
    }
    setEntered(false);
    const id = setTimeout(() => setMounted(false), SLIDE_MS);
    return () => clearTimeout(id);
  }, [open]);

  const sheetRef = useRef<HTMLElement | null>(null);
  const closeRef = useRef<HTMLButtonElement | null>(null);
  // En åpen bekreftelsesdialog eier Escape selv.
  useSheetBehavior({
    open,
    onClose: closeExportSheet,
    sheet: sheetRef,
    initialFocus: closeRef,
    escapeBlocked: () => activeDialog.peek() !== null,
    returnFocus: () =>
      document.querySelector<HTMLElement>('[data-testid="editor-export-open"]'),
  });

  // Innholdet kommer i treet ETTER at arket er åpnet, så hooken over finner
  // ikke lukkeknappen første gang: fokuset flyttes inn her.
  useEffect(() => {
    if (open && mounted) closeRef.current?.focus();
  }, [open, mounted]);

  if (!mounted) return null;
  return (
    <>
      <button
        type="button"
        class={`${styles.scrim} ${entered && open ? styles.scrimOn : ""}`}
        data-testid="export-backdrop"
        aria-label={t("app.export.close")}
        tabIndex={-1}
        onClick={closeExportSheet}
      />
      <section
        ref={sheetRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="export-sheet-title"
        data-testid="export-page"
        data-state={loadState.value}
        data-open={open ? "true" : "false"}
        inert={!open}
        class={`${styles.sheet} ${entered && open ? styles.sheetOpen : ""}`}
      >
        <header class={styles.sheetHead}>
          <span class={styles.sheetTitle}>
            <Icon name="download" size={20} />
            <h2 id="export-sheet-title" class={styles.sheetH}>
              {t("app.export.title")}
            </h2>
          </span>
          <button
            ref={closeRef}
            type="button"
            class={styles.sheetClose}
            data-testid="export-close"
            aria-label={t("app.export.close")}
            title={t("app.export.close")}
            onClick={closeExportSheet}
          >
            <Icon name="x" size={20} strokeWidth={2} />
          </button>
        </header>
        <div class={styles.sheetBody}>
          <Head />
          {exportedPath.value ? (
            <Receipt />
          ) : exporting.value ? (
            <Running />
          ) : (
            <Choices />
          )}
        </div>
      </section>
    </>
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

/** Formatkortene, i rekkefølge. MP4 bare når kilden har bilde. */
const FORMAT_CARDS = ["mp3", "wav"] as const;
type CardId = (typeof FORMAT_CARDS)[number] | "mp4";

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

      <FormatCards />

      <div class={styles.tiles}>
        <PodcastTile />
        {JINGLE_SLOTS.map((slot) => (
          <JingleTile key={slot} slot={slot} video={video} />
        ))}
      </div>
      {video ? (
        <p data-testid="export-jingles-video-note" class={styles.hint}>
          {t("app.editor.jingleVideoNote")}
        </p>
      ) : null}

      <div class={styles.field}>
        <span class={styles.label}>{t("app.editor.exWhere")}</span>
        <RadioCards
          testId="editor-dest"
          value={picked ? "pick" : "same"}
          options={destinations}
          columns={1}
          onChange={(next) => {
            if (next === "same") exportFolder.value = null;
            else void pickExportFolder();
          }}
        />
      </div>

      <Content />

      <div class={styles.foot}>
        <span data-testid="editor-export-preview" class={styles.preview}>
          {[name, mb === null ? "" : tf("app.editor.about", { mb }), folderName]
            .filter(Boolean)
            .join(DOT)}
        </span>
        <button
          type="button"
          class={styles.save}
          data-testid="editor-export-go"
          onClick={() => void runExport(kept, bytes)}
        >
          <span>{t("app.editor.saveBtn")}</span>
        </button>
      </div>
    </div>
  );
}

/**
 * Formatkortene: MP3 «Podkast», WAV «Arkiv» og — bare når kilden har bilde —
 * MP4 «Video». MP4-kortet ER «ta med video»: valgt ⇒ containeren er mp4 og
 * jinglene er dempet (motoren legger dem rundt lydsporet, ikke bildet).
 */
function FormatCards() {
  const withVideo = sourceHasVideo();
  const selected: CardId | null = isVideoExport()
    ? "mp4"
    : (FORMAT_CARDS as readonly string[]).includes(exportFormat.value)
      ? (exportFormat.value as CardId)
      : null;
  const cards: CardId[] = withVideo
    ? [...FORMAT_CARDS, "mp4"]
    : [...FORMAT_CARDS];

  function pick(id: CardId): void {
    if (id === "mp4") {
      includeVideo.value = true;
      return;
    }
    includeVideo.value = false;
    exportFormat.value = id as ExportFormat;
  }

  return (
    <div class={styles.field}>
      <span id="export-format-label" class={styles.label}>
        {t("app.editor.exFormat")}
      </span>
      <div
        role="radiogroup"
        aria-labelledby="export-format-label"
        data-testid="editor-format"
        data-count={cards.length}
        class={styles.formats}
      >
        {cards.map((id) => (
          <button
            key={id}
            type="button"
            role="radio"
            aria-checked={selected === id}
            data-testid={`editor-format-row-${id}`}
            data-selected={selected === id ? "true" : undefined}
            title={tDyn("app.editor.fmtDesc", id)}
            class={`${styles.formatCard} ${selected === id ? styles.on : ""}`}
            onClick={() => pick(id)}
          >
            {/* Formatnavnene er PRODUKTNAVN — «MP3» heter MP3 på begge språk. */}
            <b class={styles.formatName}>{id.toUpperCase()}</b>
            <span class={styles.formatTag}>
              {tDyn("app.editor.fmtTag", id)}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

/** Podkast-flisen: samme valg som Lyd-steget (`podcastAudio`). */
function PodcastTile() {
  const on = podcastAudio.value;
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={t("app.editor.podcastAudio")}
      title={t("app.editor.podcastAudioDesc")}
      data-testid="export-podcast-toggle"
      class={`${styles.tile} ${on ? styles.on : ""}`}
      onClick={() => setPodcastAudio(!on)}
    >
      <Icon name="wand" size={26} />
      <span class={styles.tileWord}>{t("app.editor.podcastTile")}</span>
      <span class={styles.tileValue}>{t("app.editor.podcastLufs")}</span>
    </button>
  );
}

/**
 * Intro eller outro.
 *
 * Uten fil: «+ Velg fil», og trykk åpner velgeren (Rust åpner den, og lagrer
 * valget i innstillingene — siden sender ingenting og får innstillingene
 * tilbake). Med fil: flisen viser filnavnet og trykk slår den av og på, og en
 * liten rund mappeknapp i hjørnet bytter fil. Med video er flisen dempet og
 * forklaringen står under: motoren legger jinglene rundt lydsporet.
 */
function JingleTile({ slot, video }: { slot: JingleSlot; video: boolean }) {
  const name = jingleName(settings.value[jingleField(slot)]);
  const onSignal = jingleOnSignal(slot);
  const on = name !== null && onSignal.value && !video;
  const which =
    slot === "intro"
      ? t("app.editor.jingleIntro")
      : t("app.editor.jingleOutro");
  const reason = video ? t("app.editor.jingleVideoNote") : undefined;
  void introOn.value;
  void outroOn.value;

  return (
    <div class={styles.tileWrap} data-testid={`export-jingle-${slot}`}>
      <button
        type="button"
        aria-disabled={video ? "true" : undefined}
        aria-pressed={name === null ? undefined : on}
        aria-label={
          name === null
            ? tf("app.editor.jinglePickLabel", { which })
            : `${which}: ${name}`
        }
        title={reason}
        data-testid={`export-jingle-${slot}-tile`}
        data-on={on ? "true" : "false"}
        class={`${styles.tile} ${on ? styles.on : ""}`}
        onClick={() => {
          if (video) return;
          if (name === null) void pickJingle(slot);
          else onSignal.value = !onSignal.value;
        }}
      >
        <Icon name="music" size={26} />
        <span class={styles.tileWord}>{which}</span>
        <span
          class={styles.tileValue}
          data-testid={`export-jingle-${slot}-name`}
          data-chosen={name ? "true" : "false"}
        >
          {name ?? t("app.editor.jingleChoose")}
        </span>
      </button>
      {name !== null ? (
        <button
          type="button"
          class={styles.swap}
          aria-disabled={video ? "true" : undefined}
          aria-label={tf("app.editor.jingleSwapLabel", { which })}
          title={reason ?? t("app.editor.jingleSwap")}
          data-testid={`export-jingle-${slot}-pick`}
          onClick={() => {
            if (!video) void pickJingle(slot);
          }}
        >
          <Icon name="folder" size={16} strokeWidth={2} />
        </button>
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
    <Card tone="good" testId="editor-exported" description={meta}>
      {/* «Lagret» med en hake som spretter inn (`pop`). */}
      <div class={styles.saved} data-testid="editor-saved">
        <span class={styles.savedCheck}>
          <Icon name="check" size={22} strokeWidth={2.4} />
        </span>
        <b>{t("app.editor.saved")}</b>
      </div>
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
