/**
 * Editoren — arbeidsflaten når et opptak er åpent på OPPTAK-siden.
 *
 * Canvasens sett 4: bølgeformen, to håndtak for hvor det du beholder begynner
 * og slutter, og kuttverktøyene — og et lydsteg. ÉN skjerm i stedet for
 * legacys 47 kontroller i tre faner pluss en eksportmodal.
 *
 * ## Hvem monterer denne, og når
 *
 * `Shell` gjør det, og bare når det er en fil på gang: OPPTAK viser LISTA til
 * `loadState` forlater `idle`. Pilen øverst til venstre («Opptak») lukker fila
 * og lista står der igjen — samme side, ingen navigering.
 *
 * ## Eksporter er et ark (F5)
 *
 * Eksporten var en egen destinasjon (D3). Nå er den knappen «Eksporter» nederst,
 * som åpner `ExportSheet` over editoren. Arket og editoren deler signalene på
 * modulnivå (`export.ts`, `model.ts`), så en eksport som går overlever at
 * arket lukkes. Editoren bak settes `inert` mens arket er åpent.
 *
 * ## Stegstripa har to
 *
 * Klipp og Lyd. Navigasjonen mellom dem er FRI: begge er klikkbare hele tiden.
 *
 ## Haken betyr «du har svart», ikke «det er en verdi her»
 *
 *   **Klipp** ✓ når det finnes kutt.
 *   **Lyd** ✓ når brukeren har VÆRT der. «Tale» er standarden, og en hake fra
 *   første sekund ville påstått at noen bestemte seg — det er nettopp forskjellen
 *   mellom en standardverdi og et valg.
 *
 * ## Bare steg 1 har bølgeform og transport
 *
 * Canvasens 4.2 og 4.3 har ingen av delene, og det er riktig: de svarer på
 * andre spørsmål. Å forlate steget river derfor lerretet ned — `WaveformHost`
 * rydder etter seg (`cancelDraw`, `ResizeObserver`), toppene blir liggende i
 * `E`, og en retur tegner dem opp igjen uten å laste noe på nytt.
 *
 * ## Slippsonen er alltid montert
 *
 * Tauri fanger OS-drag selv, og api-shimmen sender dem tilbake inn i DOM-en
 * som syntetiske `dragover`/`drop` mot `document.elementFromPoint(…)`, med
 * `File.path` satt. Sonen er derfor et element som ALLTID står — ikke en
 * overlay som dukker opp ved `dragenter`, for den finnes ikke å treffe når
 * hendelsen kommer.
 *
 * Filtypen sjekkes ikke her. Lasteren PRØVER fila og sier ærlig fra når den
 * ikke kunne leses — en fjerde kopi av lista over lydformater (api-shimmen har
 * én, legacys editor to) ville vært en fjerde ting å drifte fra hverandre.
 */

import { useEffect } from "preact/hooks";

import { locale, t, tDyn, tf } from "../i18n";
import { spanOfSeconds } from "../pages/record/record-core";
import { spanText } from "../pages/record/span-text";
import { Banner } from "../ui/Banner/Banner";
import { ActionBar } from "../ui/ActionBar/ActionBar";
import { Button } from "../ui/Button/Button";
import { Icon } from "../ui/Icon/Icon";
import { Tabs } from "../ui/Tabs/Tabs";
import {
  canRedo,
  canUndo,
  clearCuts,
  deleteCut,
  redoCut,
  undoCut,
} from "./cuts";
import { deadChannel } from "./channel-check";
import { confirmDiscard } from "./discard";
import { exactSpan, keptSeconds, timecode } from "./editor-core";
import { exporting } from "./export";
import { ExportSheet, exportSheetOpen, openExportSheet } from "./ExportSheet";
import { Loading, LoadFailed } from "./LoadStates";
import { closeFile } from "./loader";
import {
  activeStep,
  cuts,
  dirty,
  duration,
  fileName,
  loadError,
  loadState,
  playbackSource,
  startedAtMs,
  type Step,
} from "./model";
import { byTestId, focusAfterRender } from "./focus";
import { teardownPlayback } from "./playback";
import {
  REDO_KEYS_ARIA,
  redoShortcutHint,
  UNDO_KEYS_ARIA,
  undoShortcutHint,
} from "./shortcut-labels";
import {
  acceptAllSuggestions,
  suggestions,
  suggestionsEmpty,
  suggestSilences,
} from "./silence-suggest";
import { soundVisited } from "./sound";
import { SoundStep } from "./SoundStep";
import { spanLabel } from "./span";
import { resultLine } from "./summary";
import { Transport, VideoStage, hasVideo } from "./Transport";
import { WaveformHost } from "./WaveformHost";
import { longDateTitle } from "@lib/ui/date-title";
import { DOT } from "@lib/ui/dot";
import styles from "./editor.module.css";

/**
 * Overskriften Rediger skal ha.
 *
 * Datoen, som i Bibliotek — det er den frivillige kjenner opptaket sitt på.
 * Uten en dato er filnavnet tittelen, og uten en fil er det destinasjonens
 * eget ord for flaten.
 */
export function editorHeading(): string {
  const at = startedAtMs.value;
  const name = fileName.value;
  if (at === null) return name || t("nav.editor");
  return longDateTitle(at, locale.value);
}

export function EditorPage() {
  const state = loadState.value;

  // Å gå et annet sted STOPPER avspillingen, men lukker ikke fila. Legacys
  // `deactivateEditor` gjør nøyaktig det, og kommentaren over den sier hvorfor
  // den andre halvdelen ikke skjer: å slippe topper, kutt og forslag ved et
  // sidebytte ga en tom bølgeform når man kom tilbake, og brukeren måtte lukke
  // og åpne fila på nytt for å se noe (rapportert feil, mai 2026). Uten den
  // FØRSTE halvdelen ville lyden gått videre i bakgrunnen på en side ingen ser. Motoren lukkes
  // også: lydkortet har én eier om gangen, og den eieren er den siden som er synlig.
  useEffect(() => teardownPlayback, []);

  return (
    <div
      data-testid="editor"
      data-state={state}
      // Grunnen til at åpningen feilet, som et attributt: den er en NØKKEL og
      // ikke en setning, så den hører ikke hjemme i treet der en skjermleser
      // ville lest den opp. Banneret sier det samme på norsk.
      data-reason={loadError.value ?? undefined}
      class={styles.page}
    >
      {state === "ready" ? (
        <Workspace />
      ) : state === "loading" ? (
        <Loading />
      ) : (
        // `idle` kommer aldri hit: skallet viser biblioteket da. Feilen er
        // derfor den eneste andre muligheten, og en `else` som het `idle`
        // ville vært en gren ingen kan nå og ingen kan teste.
        <LoadFailed />
      )}
    </div>
  );
}

// ── Arbeidsflaten ───────────────────────────────────────────────────────────

function Workspace() {
  const step = activeStep.value;
  const cutAnswered = cuts.value.length > 0;

  const sheet = exportSheetOpen.value;

  // Arket lever bare så lenge fila er åpen: lukkes fila, rives hele
  // arbeidsflaten (og arket med den), og signalet må ikke stå sant til neste
  // fil.
  useEffect(() => () => void (exportSheetOpen.value = false), []);

  return (
    <>
      <div
        class={styles.workspace}
        data-testid="editor-workspace"
        // `inert` bak arket: fokus og klikk kommer ikke ut av det.
        inert={sheet}
      >
        <Head />
        <Tabs
          label={t("editor.tabsAria")}
          testId="editor-steps"
          value={step}
          onChange={(id) => (activeStep.value = id as Step)}
          items={[
            {
              id: "cut",
              label: t("app.editor.stepCut"),
              step: "1",
              done: cutAnswered,
            },
            {
              id: "sound",
              label: t("app.editor.stepSound"),
              step: "2",
              done: soundVisited.value,
            },
          ]}
        />
        <DeadChannelNotice />
        {step === "cut" ? <CutStep /> : <SoundStep />}
        <NextStep step={step} />
      </div>
      <ExportSheet open={sheet} />
    </>
  );
}

/**
 * Steg 1 (E7): bildet først når opptaket har video, så bølgeformen.
 *
 * Med video står et stort 16:9-bilde midt på med transporten i en mørk stripe
 * nederst i bildet, og bølgeformen er lavere under. Uten video er det som før:
 * bølgeformen er stor og transporten står under den.
 */
function CutStep() {
  // Å gå til et annet STEG stopper avspillingen, av nøyaktig samme grunn som å
  // gå til en annen SIDE gjør det: lyd som går videre på en skjerm ingen ser
  // er den ene formen for feil som ikke ser ut som en feil.
  useEffect(() => teardownPlayback, []);
  const video = hasVideo();

  return (
    <>
      <PlaybackNotice />
      {video ? <VideoStage /> : null}
      <WaveformHost compact={video} />
      {video ? null : <Transport />}
      <CutTools />
    </>
  );
}

/**
 * Handlingslinja: «Neste: Lyd» på Klipp-steget, og «Eksporter» alltid.
 *
 * «Eksporter» åpner `ExportSheet` over editoren — fila blir stående åpen, og
 * signalene bak arket bor på modulnivå. «Neste: Lyd» er veien VIDERE for den
 * som følger stegene; stripa øverst er navigasjon for den som vet hvor hun
 * skal.
 */
function NextStep({ step }: { step: Step }) {
  return (
    <ActionBar testId="editor-actions">
      {step === "cut" ? (
        <Button
          variant="secondary"
          testId="editor-next"
          onClick={() => (activeStep.value = "sound")}
        >
          {t("app.editor.nextSound")}
        </Button>
      ) : null}
      <Button
        variant="primary"
        size="lg"
        testId="editor-export-open"
        onClick={openExportSheet}
      >
        {exporting.value
          ? t("editor.exportExporting")
          : t("app.editor.exportOpen")}
      </Button>
    </ActionBar>
  );
}

function Head() {
  const total = duration.value;
  // Bibliotekets form, ikke resultatlinjas: «hvor lenge varte opptaket» er
  // ikke et sekundspørsmål, og raden i Bibliotek sier det samme om det samme
  // opptaket. Sekundene hører hjemme i RESULTATET, der ti sekunder er
  // forskjellen på å ha med velsignelsen eller ikke.
  //
  // Filnavnet står bare her når overskriften er noe annet (datoen): er det
  // selve overskriften, er det å gjenta det støy.
  const name = fileName.value;
  const sub = [
    name && name !== editorHeading() ? name : "",
    spanText(spanOfSeconds(total)),
  ]
    .filter(Boolean)
    .join(DOT);

  return (
    <div class={styles.head}>
      {/*
        Pila tilbake til lista. `closeFile` stiller selv eksport-spørsmålet
        (F2-3) og kan svare nei, så den er ventet på og ikke fyrt av. Lista
        står på samme side i samme øyeblikk fila lukkes — ingen `navigate`.
      */}
      <Button
        variant="ghost"
        testId="editor-close"
        icon={<span aria-hidden="true">←</span>}
        onClick={() => {
          void confirmDiscard().then((ok) => {
            if (ok) void closeFile();
          });
        }}
      >
        {t("app.editor.toLibrary")}
      </Button>
      <p data-testid="editor-sub" class={styles.sub}>
        <span>{sub}</span>
        <Summary />
        {dirty.value ? (
          <span
            data-testid="editor-dirty"
            class={styles.dot}
            title={t("tooltip.unsavedChanges")}
            role="img"
            aria-label={t("tooltip.unsavedChanges")}
          />
        ) : null}
      </p>
    </div>
  );
}

/**
 * Resultatet, på det steget som ikke har en resultatlinje av seg selv.
 *
 * Steg 1 har den i transportlinja, ved siden av bølgeformen den beskriver.
 * Steg 2 har ingen bølgeform, og «hva er det egentlig jeg sitter igjen med» er
 * det eneste tallet som betyr noe der. Setningen bygges i `summary.ts`, som
 * eksportarket også leser — to steder som regnet ut den samme
 * varigheten er to steder som kan bli uenige om den.
 */
function Summary() {
  if (activeStep.value === "cut") return null;
  return (
    <span data-testid="editor-summary">
      {DOT}
      {resultLine()}
    </span>
  );
}

/** «Venstre side er stille — sjekk kabelen», når analysen fant en død side. Ingen reparasjon. */
function DeadChannelNotice() {
  const side = deadChannel.value;
  if (!side) return null;
  return (
    <Banner
      tone="warn"
      testId="editor-dead-channel"
      title={tDyn("app.editor.deadChannel", side)}
    />
  );
}

/**
 * Én ærlig setning om avspillingen, eller ingen.
 *
 * `original` sier ingenting — det er normaltilstanden. `none` betyr at avspillingsmotoren ikke
 * lot seg åpne (ingen lydenhet, eller fila lot seg ikke lese), og da står setningen her i
 * stedet for at knappen ikke gjør noe. I en ren nettleser finnes ingen motor, så det er dét man
 * ser der.
 */
function PlaybackNotice() {
  if (playbackSource.value === "original") return null;
  return (
    <Banner
      tone="warn"
      testId="editor-playback-notice"
      title={t("editor.qualityFallback")}
    />
  );
}

const REMOVE_SELECTOR = '[data-testid="editor-cut-remove"]';

/** «Ferdig lengde: 28 min av 1 t 20 min» — resultatet av kuttene. */
function ResultLine() {
  const total = duration.value;
  const kept = keptSeconds(cuts.value, total);
  return (
    <span data-testid="editor-result" class={styles.result}>
      {tf("app.editor.result", {
        kept: spanLabel(exactSpan(kept)),
        total: spanLabel(exactSpan(total)),
      })}
    </span>
  );
}

/** Kuttverktøyene: angre, gjør om, fjern alle, og lista over kutt. */
function CutTools() {
  const list = cuts.value;
  const suggested = suggestions.value;
  const emptyResult = suggestionsEmpty.value;

  return (
    <div data-testid="editor-tools" class={styles.tools}>
      <p class={styles.hint}>{t("editor.dragHint")}</p>
      <ResultLine />
      <div class={styles.toolbar}>
        <Button
          variant="secondary"
          testId="editor-undo"
          disabled={!canUndo.value}
          disabledReason={t("app.editor.nothingToUndo")}
          shortcutHint={undoShortcutHint()}
          keyShortcuts={UNDO_KEYS_ARIA}
          onClick={undoCut}
        >
          {t("trash.undo")}
        </Button>
        <Button
          variant="secondary"
          testId="editor-redo"
          disabled={!canRedo.value}
          disabledReason={t("app.editor.nothingToRedo")}
          shortcutHint={redoShortcutHint()}
          keyShortcuts={REDO_KEYS_ARIA}
          onClick={redoCut}
        >
          {t("app.editor.redo")}
        </Button>
        <Button
          variant="secondary"
          testId="editor-silence"
          icon={<Icon name="wand" size={18} />}
          onClick={suggestSilences}
        >
          {t("app.editor.silence")}
        </Button>
        {suggested.length > 0 ? (
          <Button
            variant="primary"
            testId="editor-silence-all"
            onClick={() => {
              acceptAllSuggestions();
              focusAfterRender(byTestId('[data-testid="editor-silence"]'));
            }}
          >
            {tf("app.editor.silenceAll", { n: suggested.length })}
          </Button>
        ) : null}
        <Button
          variant="ghost"
          testId="editor-clear-cuts"
          disabled={list.length === 0}
          disabledReason={t("app.editor.nothingToUndo")}
          onClick={clearCuts}
        >
          {t("editor.cutsNone")}
        </Button>
      </div>
      {suggested.length === 0 && emptyResult ? (
        <p data-testid="editor-silence-none" class={styles.hint}>
          {t("app.editor.silenceNone")}
        </p>
      ) : null}
      {list.length === 0 ? null : (
        <>
          <span class={styles.label}>{t("editor.cutsTitle")}</span>
          <ul data-testid="editor-cut-list" class={styles.cutList}>
            {list.map((cut, index) => (
              <li
                key={`${cut.start}:${cut.end}`}
                data-testid="editor-cut-row"
                class={styles.cutRow}
              >
                <span data-testid="editor-cut-range" class={styles.cutRange}>
                  {`${timecode(cut.start)} – ${timecode(cut.end)}`}
                </span>
                <span class={styles.cutSpan}>
                  {spanLabel(exactSpan(cut.end - cut.start))}
                </span>
                <Button
                  variant="ghost"
                  testId="editor-cut-remove"
                  onClick={() => {
                    deleteCut(index);
                    // Raden er borte: neste rads «Fjern», ellers forrige,
                    // ellers «Fjern alle» — aldri `<body>`.
                    focusAfterRender(
                      () =>
                        document.querySelectorAll(REMOVE_SELECTOR)[index] ??
                        null,
                      () =>
                        document.querySelectorAll(REMOVE_SELECTOR)[index - 1] ??
                        null,
                      byTestId('[data-testid="editor-clear-cuts"]'),
                    );
                  }}
                >
                  {t("editor.deleteCut")}
                </Button>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
