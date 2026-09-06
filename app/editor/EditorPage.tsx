/**
 * REDIGER — arbeidsflaten når en fil er åpen, steg 1: KLIPP.
 *
 * Canvasens sett 4, artboard 4.1. Legacys editor har 47 kontroller i tre faner
 * pluss en eksportmodal med 25 til; her er det ÉN skjerm med ett spørsmål:
 * *er dette prekenen?* Svaret er ett klikk, og alt annet er tilgjengelig for
 * den som vil ha det.
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
 *   **Klipp** ✓ når spørsmålet er besvart: kutt finnes, eller «Behold bare
 *   prekenen» / «Behold alt» er brukt.
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
 * ## Forslagskortet vises bare når det finnes et forslag
 *
 * Fant ikke analysen noen preken, står det ingenting der — ikke et tomt kort
 * og ikke en unnskyldning. Til gjengjeld åpnes kuttverktøyene av seg selv i
 * det tilfellet: skjermen tilbyr det den KAN gjøre, i stedet for å be om et
 * klikk for å avsløre den eneste veien videre.
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
import { Button } from "../ui/Button/Button";
import { Card } from "../ui/Card/Card";
import { Select } from "../ui/Select/Select";
import { Tabs } from "../ui/Tabs/Tabs";
import {
  applySermon,
  canRedo,
  canUndo,
  clearCuts,
  deleteCut,
  keepAll,
  redoCut,
  undoCut,
} from "./cuts";
import { confirmDiscard } from "./discard";
import {
  exactSpan,
  keptSeconds,
  suggestionIsWorthOffering,
  timecode,
} from "./editor-core";
import { Loading, LoadFailed } from "./LoadStates";
import { closeFile } from "./loader";
import {
  activeStep,
  analyzing,
  applied,
  cuts,
  dirty,
  dismissed,
  duration,
  fileName,
  loadError,
  loadState,
  manualMode,
  playbackSource,
  playheadSec,
  playing,
  segments,
  startedAtMs,
  suggestion,
  type Step,
} from "./model";
import { stopPlay, togglePlay } from "./playback";
import { candidatesFor, chooseSermon } from "./sermon";
import { soundVisited } from "./sound";
import { SoundStep } from "./SoundStep";
import { spanLabel } from "./span";
import { resultLine } from "./summary";
import { WaveformHost } from "./WaveformHost";
import { longDateTitle } from "@lib/ui/date-title";
import { DOT } from "@lib/ui/dot";
import styles from "./editor.module.css";

/** Merket foran blokken som ER valgt. Et symbol, ikke tekst å oversette. */
const STAR = "★ ";

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
  // FØRSTE halvdelen ville lyden gått videre i bakgrunnen på en side ingen ser.
  useEffect(() => stopPlay, []);

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
  const cutAnswered = applied.value || dismissed.value || cuts.value.length > 0;

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
  useEffect(() => stopPlay, []);

  return (
    <>
      <PlaybackNotice />
      <SuggestionCard />
      <WaveformHost />
      <Transport />
      <ManualTools />
      <SermonPicker />
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
    <div class={styles.nextRow}>
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
    </div>
  );
}

function Head() {
  const total = duration.value;
  // Bibliotekets form, ikke resultatlinjas: «hvor lenge varte opptaket» er
  // ikke et sekundspørsmål, og raden i Bibliotek sier det samme om det samme
  // opptaket. Sekundene hører hjemme i RESULTATET, der ti sekunder er
  // forskjellen på å ha med velsignelsen eller ikke.
  const sub = [fileName.value, spanText(spanOfSeconds(total))]
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
            closeFile();
          });
        }}
      >
        {t("app.editor.toLibrary")}
      </Button>
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
 * `original` sier ingenting — det er normaltilstanden. `proxy` og `none` er
 * legacys egne tekster, og de finnes derfor i alle sju språk. Atlasets
 * forbehold gjelder fortsatt: i en ren nettleser er `asset://` død, så
 * `none`-banneret er dét man ser der, og det er sant.
 */
function PlaybackNotice() {
  const source = playbackSource.value;
  if (source === "original") return null;
  return (
    <Banner
      tone="warn"
      testId="editor-playback-notice"
      title={
        source === "proxy"
          ? t("editor.playbackViaProxy")
          : t("editor.qualityFallback")
      }
    />
  );
}

/**
 * «Vi tror prekenen er her».
 *
 * Vises bare når analysen fant noe OG det er noe å trimme OG ingen har svart
 * på det ennå. Ellers ingenting — se toppen av fila.
 */
function SuggestionCard() {
  const range = suggestion.value;
  const total = duration.value;
  if (applied.value || dismissed.value) return null;
  if (analyzing.value) {
    return (
      <p data-testid="editor-searching" class={styles.hint}>
        {t("app.editor.searching")}
      </p>
    );
  }
  if (!suggestionIsWorthOffering(range, total) || !range) return null;

  return (
    <Card
      tone="selected"
      testId="editor-suggestion"
      title={t("app.editor.found")}
      description={tf("app.editor.foundDesc", {
        from: timecode(range.start),
        to: timecode(range.end),
        span: spanLabel(exactSpan(range.end - range.start)),
      })}
      actions={
        <>
          <Button
            variant="primary"
            testId="editor-keep-sermon"
            onClick={applySermon}
          >
            {t("app.editor.keepSermon")}
          </Button>
          <Button
            variant="secondary"
            testId="editor-keep-all"
            onClick={keepAll}
          >
            {t("app.editor.keepAll")}
          </Button>
        </>
      }
    />
  );
}

/** Spill/pause, klokka, og resultatet. */
function Transport() {
  const total = duration.value;
  const kept = keptSeconds(cuts.value, total);
  const dead = playbackSource.value === "none";

  return (
    <div class={styles.transport}>
      <button
        type="button"
        class={styles.play}
        data-testid="editor-play"
        aria-disabled={dead ? "true" : undefined}
        aria-label={playing.value ? t("app.editor.pause") : t("tooltip.play")}
        title={dead ? t("editor.qualityFallback") : undefined}
        onClick={() => {
          if (dead) return;
          togglePlay();
        }}
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
      <Button
        variant="ghost"
        testId="editor-manual"
        onClick={() => (manualMode.value = !manualMode.value)}
      >
        {manualMode.value ? t("app.editor.manualHide") : t("app.editor.manual")}
      </Button>
    </div>
  );
}

/**
 * Kuttverktøyene, avslørt av «Klipp manuelt».
 *
 * `manualMode` er den ENE sannheten om hvorvidt de står. To andre steder skrur
 * den PÅ: «Behold bare prekenen» (så det man fjernet er synlig og Angre er
 * innen rekkevidde) og en analyse som ikke fant noen preken (da er dette den
 * eneste veien videre). Begge skriver flagget i stedet for å legge til en
 * betingelse her — to skrivere på ett flagg er greit; to REGLER om det samme
 * flagget er skjøten dette skallet er skrevet for å unngå.
 */
function ManualTools() {
  const list = cuts.value;
  if (!manualMode.value) return null;

  return (
    <div data-testid="editor-tools" class={styles.tools}>
      <p class={styles.hint}>{t("editor.dragHint")}</p>
      <div class={styles.toolbar}>
        <Button
          variant="secondary"
          testId="editor-undo"
          disabled={!canUndo.value}
          disabledReason={t("app.editor.nothingToUndo")}
          onClick={undoCut}
        >
          {t("trash.undo")}
        </Button>
        <Button
          variant="secondary"
          testId="editor-redo"
          disabled={!canRedo.value}
          disabledReason={t("app.editor.nothingToRedo")}
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
                  onClick={() => deleteCut(index)}
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

/**
 * «Er ikke dette prekenen?»
 *
 * Vises bare når det finnes mer enn ett troverdig alternativ — en liste med
 * ett valg er ikke et valg. Blokkene og indeksene kommer fra
 * `@lib/pages/editor/sermon-candidates`, som er den ENE lista både tilbudet og
 * korreksjonen bygges på: to lister var nettopp feilen som lot en korreksjon
 * lande på feil segment, stille.
 */
function SermonPicker() {
  const list = segments.value;
  const candidates = candidatesFor(list);
  if (candidates.length < 2) return null;
  const chosen = candidates.find(
    (c) => list[c.index]?.type === "sermon",
  )?.index;

  return (
    <div class={styles.picker}>
      <span id="editor-picker-label" class={styles.label}>
        {t("editor.sermonPickerLabel")}
      </span>
      <span id="editor-picker-desc" class={styles.hint}>
        {t("app.editor.notSermonDesc")}
      </span>
      <Select
        testId="editor-picker"
        labelId="editor-picker-label"
        describedBy="editor-picker-desc"
        value={String(chosen ?? candidates[0].index)}
        onChange={(next) => chooseSermon(Number(next))}
        options={candidates.map((candidate, position) => ({
          value: String(candidate.index),
          label:
            (candidate.index === chosen ? STAR : "") +
            tf("app.editor.blockOption", {
              n: position + 1,
              at: timecode(candidate.segment.start),
              span: spanLabel(exactSpan(candidate.segment.duration)),
            }),
        }))}
      />
    </div>
  );
}
