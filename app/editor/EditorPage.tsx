/**
 * REDIGER — arbeidsflaten når en fil er åpen, steg 1: KLIPP.
 *
 * Canvasens sett 4, artboard 4.1. Legacys editor har 47 kontroller i tre faner
 * pluss en eksportmodal med 25 til; her er det ÉN skjerm: bølgeformen, to
 * håndtak for hvor det du beholder begynner og slutter, og kuttverktøyene.
 *
 * ## Hvem monterer denne, og når
 *
 * `Shell` gjør det, og bare når det er en fil på gang: REDIGERING-destinasjonen
 * viser BIBLIOTEKET til `loadState` forlater `idle`. Denne fila har derfor
 * ingen tomtilstand lenger — biblioteket ER tomtilstanden, og en tom skjerm med
 * «dra et opptak hit» ved siden av en liste over alle opptakene var to svar på
 * det samme spørsmålet.
 *
 * ## Stegstripa har to
 *
 * D3 flyttet EKSPORTER ut til sin egen destinasjon: mastering og miksing skal
 * kunne bo der på sikt, og et steg inne i et annet steg er ikke et sted noe kan
 * vokse. «Neste: Eksporter» på steg 2 navigerer dit.
 *
 * Navigasjonen mellom de to er FRI: begge er klikkbare hele tiden. En som har
 * vært innom lyden skal kunne gå tilbake og klippe litt til.
 *
 * ## Haken betyr «du har svart», ikke «det er en verdi her»
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

import { locale, t, tf } from "../i18n";
import { spanOfSeconds } from "../pages/record/record-core";
import { spanText } from "../pages/record/span-text";
import { navigate } from "../router/router";
import { Banner } from "../ui/Banner/Banner";
import { ActionBar } from "../ui/ActionBar/ActionBar";
import { Button } from "../ui/Button/Button";
import { Tabs } from "../ui/Tabs/Tabs";
import {
  canRedo,
  canUndo,
  clearCuts,
  deleteCut,
  redoCut,
  undoCut,
} from "./cuts";
import { confirmDiscard } from "./discard";
import { exactSpan, keptSeconds, timecode } from "./editor-core";
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
  playheadSec,
  playing,
  startedAtMs,
  type Step,
} from "./model";
import { byTestId, focusAfterRender } from "./focus";
import { teardownPlayback, togglePlay } from "./playback";
import {
  PLAY_KEYS_ARIA,
  playShortcutHint,
  REDO_KEYS_ARIA,
  redoShortcutHint,
  UNDO_KEYS_ARIA,
  undoShortcutHint,
} from "./shortcut-labels";
import { soundVisited } from "./sound";
import { SoundStep } from "./SoundStep";
import { spanLabel } from "./span";
import { resultLine } from "./summary";
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

  return (
    <>
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
      {step === "cut" ? <CutStep /> : <SoundStep />}
      <NextStep step={step} />
    </>
  );
}

/** Steg 1 — canvasens 4.1, uendret fra P4a. */
function CutStep() {
  // Å gå til et annet STEG stopper avspillingen, av nøyaktig samme grunn som å
  // gå til en annen SIDE gjør det: lyd som går videre på en skjerm ingen ser
  // er den ene formen for feil som ikke ser ut som en feil.
  useEffect(() => teardownPlayback, []);

  return (
    <>
      <PlaybackNotice />
      <WaveformHost />
      <Transport />
      <CutTools />
    </>
  );
}

/**
 * «Neste: Lyd» / «Neste: Eksporter».
 *
 * Canvasen tegner den nederst på 4.2; den står på 4.1 av samme grunn. Stripa
 * øverst er navigasjon for den som vet hvor hun skal — knappen nederst er veien
 * VIDERE for den som følger den, og den skal være der man er ferdig med å lese.
 *
 * D3: den siste av dem forlater SIDEN. Eksporteringen er en destinasjon nå, og
 * fila blir stående åpen — signalene bak den bor på modulnivå og overlever at
 * flaten avmonteres, så «Neste: Eksporter» er en navigasjon og ikke en
 * overlevering.
 */
function NextStep({ step }: { step: Step }) {
  return (
    <ActionBar testId="editor-actions">
      {/*
        «Til biblioteket» — og ingen navigering. Etter D3 ER dette biblioteket:
        REDIGERING viser lista igjen i samme øyeblikk fila lukkes, på den samme
        siden. En `navigate` her ville vært et rutebytte til stedet man
        allerede står, med fokusflytting og rulling som følge.
      */}
      <Button
        variant="ghost"
        testId="editor-close"
        onClick={() => {
          void confirmDiscard().then((ok) => {
            if (!ok) return;
            // `closeFile` stiller selv eksport-spørsmålet (F2-3) og kan svare
            // nei — den er derfor ventet på, ikke fyrt av.
            void closeFile();
          });
        }}
      >
        {t("app.editor.toLibrary")}
      </Button>
      <Button
        variant="primary"
        size="lg"
        testId="editor-next"
        onClick={() => {
          if (step === "cut") activeStep.value = "sound";
          else navigate("export");
        }}
      >
        {step === "cut"
          ? t("app.editor.nextSound")
          : t("app.editor.nextExport")}
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
 * EKSPORTERING-siden også leser — to steder som regnet ut den samme
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

/** Spill/pause, klokka, og resultatet. */
function Transport() {
  const total = duration.value;
  const kept = keptSeconds(cuts.value, total);

  return (
    <div class={styles.transport}>
      <button
        type="button"
        class={styles.play}
        data-testid="editor-play"
        aria-label={playing.value ? t("app.editor.pause") : t("tooltip.play")}
        title={playShortcutHint()}
        aria-keyshortcuts={PLAY_KEYS_ARIA}
        onClick={togglePlay}
      >
        {playing.value ? (
          <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <rect x="6" y="5" width="4" height="14" rx="1" />
            <rect x="14" y="5" width="4" height="14" rx="1" />
          </svg>
        ) : (
          <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <path d="M8 5v14l11-7z" />
          </svg>
        )}
      </button>
      <b data-testid="editor-time" class={styles.time}>
        {timecode(playheadSec.value)}
      </b>
      <span data-testid="editor-total" class={styles.total}>
        {timecode(total)}
      </span>
      <span data-testid="editor-result" class={styles.result}>
        {tf("app.editor.result", {
          kept: spanLabel(exactSpan(kept)),
          total: spanLabel(exactSpan(total)),
        })}
      </span>
    </div>
  );
}

/** Kuttverktøyene: angre, gjør om, fjern alle, og lista over kutt. */
function CutTools() {
  const list = cuts.value;

  return (
    <div data-testid="editor-tools" class={styles.tools}>
      <p class={styles.hint}>{t("editor.dragHint")}</p>
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
          variant="ghost"
          testId="editor-clear-cuts"
          disabled={list.length === 0}
          disabledReason={t("app.editor.nothingToUndo")}
          onClick={clearCuts}
        >
          {t("editor.cutsNone")}
        </Button>
      </div>
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
