/**
 * REDIGER — jobb nr. 3 av fire, steg 1: KLIPP.
 *
 * Canvasens sett 4, artboard 4.1. Legacys editor har 47 kontroller i tre faner
 * pluss en eksportmodal med 25 til; her er det ÉN skjerm med ett spørsmål:
 * *er dette prekenen?* Svaret er ett klikk, og alt annet er tilgjengelig for
 * den som vil ha det.
 *
 * ## Stegstripa viser bare steget som finnes
 *
 * Canvasen har tre steg — 1 Klipp · 2 Lyd · 3 Eksporter — og P4a bygger det
 * første. De to andre står IKKE i stripa, hverken som knapper eller som
 * dempede plassholdere: husregelen fra S1b er at ingenting sier «kommer
 * senere» og at ingen knapp finnes uten å gjøre noe. En dempet «2 Lyd» ville
 * vært begge deler på én gang.
 *
 * Alternativet — å droppe stripa helt til alle tre finnes — ville skjult
 * FORMEN, og formen er halve poenget: en frivillig skal se at dette er en vei
 * med et endepunkt. Så stripa står, med ett steg i, og P4b legger til to.
 * `Tabs` er allerede bygget for dette (`TabItem.done`, «editorens steg»), så
 * det er en tabellrad å legge til og ingenting annet.
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

import { useEffect, useRef, useState } from "preact/hooks";

import { locale, t, tDyn, tf } from "../i18n";
import { spanOfSeconds } from "../pages/record/record-core";
import { spanText } from "../pages/record/span-text";
import { navigate } from "../router/router";
import { lastRecording, loadRecordingCount } from "../state/recordings";
import { Banner } from "../ui/Banner/Banner";
import { Button } from "../ui/Button/Button";
import { Card } from "../ui/Card/Card";
import { EmptyState } from "../ui/EmptyState/EmptyState";
import { ProgressBar } from "../ui/ProgressBar/ProgressBar";
import { Select } from "../ui/Select/Select";
import { Tabs } from "../ui/Tabs/Tabs";
import { confirmDialog } from "../ui/dialog";
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
import {
  exactSpan,
  keptSeconds,
  suggestionIsWorthOffering,
  timecode,
} from "./editor-core";
import { closeFile, openFile, pickAndOpen } from "./loader";
import {
  activeStep,
  analyzing,
  applied,
  cuts,
  dirty,
  dismissed,
  duration,
  E,
  fileName,
  loadError,
  loadPhase,
  loadProgress,
  loadState,
  manualMode,
  playbackSource,
  playheadSec,
  playing,
  segments,
  startedAtMs,
  suggestion,
} from "./model";
import { stopPlay, togglePlay } from "./playback";
import { candidatesFor, chooseSermon } from "./sermon";
import { spanLabel } from "./span";
import { WaveformHost } from "./WaveformHost";
import styles from "./editor.module.css";

/** Skilletegn mellom fakta på én linje. Et tegn, ikke prosa. */
const DOT = " · ";
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
  const loc = locale.value;
  const when = new Date(at);
  const date = when.toLocaleDateString(loc, {
    weekday: "long",
    day: "numeric",
    month: "long",
    year: "numeric",
  });
  return date ? date[0].toLocaleUpperCase(loc) + date.slice(1) : name;
}

export function EditorPage() {
  const state = loadState.value;
  const [over, setOver] = useState(false);
  const dropRef = useRef<HTMLDivElement | null>(null);

  // Slippsonen: ETT element, alltid montert. Lytterne settes imperativt fordi
  // shimmens syntetiske hendelser er ekte `DragEvent`-er som bobler — Preacts
  // `onDrop` ville også fungert, men `dragover` MÅ ha `preventDefault()` for
  // at slippet i det hele tatt skal skje, og det er lettere å se her.
  useEffect(() => {
    const el = dropRef.current;
    if (!el) return;
    const onOver = (event: DragEvent): void => {
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
      setOver(true);
    };
    const onLeave = (): void => setOver(false);
    const onDrop = (event: DragEvent): void => {
      event.preventDefault();
      setOver(false);
      const file = event.dataTransfer?.files?.[0] as
        (File & { path?: string }) | undefined;
      const path = file?.path;
      if (!path) return;
      void openDropped(path);
    };
    el.addEventListener("dragover", onOver);
    el.addEventListener("dragleave", onLeave);
    el.addEventListener("drop", onDrop);
    return () => {
      el.removeEventListener("dragover", onOver);
      el.removeEventListener("dragleave", onLeave);
      el.removeEventListener("drop", onDrop);
      // Å gå et annet sted STOPPER avspillingen, men lukker ikke fila. Legacys
      // `deactivateEditor` gjør nøyaktig det, og kommentaren over den sier
      // hvorfor den andre halvdelen ikke skjer: å slippe topper, kutt og
      // forslag ved et fanebytte ga en tom bølgeform når man kom tilbake, og
      // brukeren måtte lukke og åpne fila på nytt for å se noe (rapportert
      // feil, mai 2026). Uten den FØRSTE halvdelen ville lyden gått videre i
      // bakgrunnen på en side ingen ser.
      stopPlay();
    };
  }, []);

  return (
    <div
      ref={dropRef}
      data-testid="editor"
      data-state={state}
      // Grunnen til at åpningen feilet, som et attributt: den er en NØKKEL og
      // ikke en setning, så den hører ikke hjemme i treet der en skjermleser
      // ville lest den opp. Banneret sier det samme på norsk.
      data-reason={loadError.value ?? undefined}
      class={`${styles.page} ${over ? styles.dropzoneOver : ""}`}
    >
      {state === "ready" ? (
        <Workspace />
      ) : state === "loading" ? (
        <Loading />
      ) : state === "error" ? (
        <LoadFailed />
      ) : (
        <Empty over={over} />
      )}
    </div>
  );
}

/**
 * Åpne en sluppet fil.
 *
 * Et slipp er en eksplisitt handling fra brukeren, så mappen får tillit for
 * denne økta — uten det avviser sti-forsvaret et opptak som ligger på en
 * ekstern disk eller et sted som ikke ligner på lagringsmappen. Legacy gjør
 * det samme, i sin egen slipp-håndterer.
 */
async function openDropped(path: string): Promise<void> {
  if (!(await confirmDiscard())) return;
  try {
    await window.api.registerTrustedPath(path);
  } catch {
    /* forsvaret svarer nei — lasteren sier ærlig fra hvis det var grunnen */
  }
  void openFile(path);
}

/** Spør før ulagrede kutt kastes. Sann = det er trygt å gå videre. */
async function confirmDiscard(): Promise<boolean> {
  if (!E.dirty) return true;
  return confirmDialog({
    title: t("editor.confirmClose"),
    message: t("dialog.discardEditsBody"),
    confirmLabel: t("dialog.discardEdits"),
    danger: true,
  });
}

// ── Tomtilstanden ───────────────────────────────────────────────────────────

function Empty({ over }: { over: boolean }) {
  const last = lastRecording.value;

  useEffect(() => {
    void loadRecordingCount();
  }, []);

  return (
    <div class={styles.drop}>
      <div class={`${styles.dropzone} ${over ? styles.dropzoneOver : ""}`}>
        <EmptyState
          testId="editor-empty"
          title={over ? t("editor.dropFile") : t("app.editor.emptyTitle")}
          description={t("app.editor.emptyDesc")}
          action={
            <Button
              variant="primary"
              testId="editor-open"
              onClick={() => void pickAndOpen()}
            >
              {t("editor.openFile")}
            </Button>
          }
        />
      </div>

      {/* Ikke lest ennå, eller ingenting å vise: ingen påstand i noen retning. */}
      {last?.path ? (
        <div class={styles.recent}>
          <span class={styles.label}>{t("app.record.last")}</span>
          <div data-testid="editor-recent" class={styles.recentRow}>
            <div class={styles.recentGrow}>
              <div class={styles.value}>{last.date || last.filename}</div>
              <div class={styles.recentName}>{last.filename}</div>
            </div>
            <Button
              variant="secondary"
              testId="editor-recent-open"
              onClick={() =>
                void openFile(last.path as string, {
                  startedAtMs: last.startedAt ?? last.timestamp ?? null,
                })
              }
            >
              {t("nav.editor")}
            </Button>
          </div>
        </div>
      ) : null}
    </div>
  );
}

// ── Lastingen ───────────────────────────────────────────────────────────────

function Loading() {
  const phase = loadPhase.value;
  const fraction = loadProgress.value;
  // Ingen fase ennå betyr at vi står i ffprobe eller i en sidevogn — begge er
  // millisekunder. «Analyserer …» er det ærlige ordet for «vi holder på», og
  // det er legacys egen tekst.
  // `tDyn` og ikke `t()` med en malstreng: prefikset er en literal gaten kan
  // slå opp, og suffikset er halvdelen ingen gate kan kjenne — så et bom
  // KASTER i DEV i stedet for å male en tom linje som ser ut som «denne er
  // visst tom».
  const text = phase ? tDyn("editor", phase) : t("editor.analyzing");

  return (
    <div data-testid="editor-loading" class={styles.loading}>
      <p data-testid="editor-loading-text" class={styles.loadingText}>
        {text}
      </p>
      {fraction === null ? null : (
        <ProgressBar
          fraction={fraction}
          label={text}
          hideReadout
          testId="editor-loading-progress"
        />
      )}
    </div>
  );
}

function LoadFailed() {
  return (
    <>
      <Banner
        tone="bad"
        testId="editor-load-error"
        title={t("app.editor.loadFailed")}
        detail={t("app.editor.loadFailedDesc")}
        actions={
          <Button
            variant="secondary"
            testId="editor-load-error-open"
            onClick={() => void pickAndOpen()}
          >
            {t("editor.openFile")}
          </Button>
        }
      />
      {/* Feilen skjuler ikke veien tilbake: `loadError` bæres bare som en
          nøkkel, og lukking setter tilstanden til `idle` igjen. */}
      <div class={styles.toolbar}>
        <Button
          variant="ghost"
          testId="editor-load-error-close"
          onClick={closeFile}
        >
          {t("editor.closeFile")}
        </Button>
      </div>
    </>
  );
}

// ── Arbeidsflaten ───────────────────────────────────────────────────────────

function Workspace() {
  const step = activeStep.value;

  return (
    <>
      <Head />
      <Tabs
        label={t("editor.tabsAria")}
        testId="editor-steps"
        value={step}
        onChange={(id) => (activeStep.value = id as typeof step)}
        items={[{ id: "cut", label: t("app.editor.stepCut"), step: "1" }]}
      />
      <PlaybackNotice />
      <SuggestionCard />
      <WaveformHost />
      <Transport />
      <ManualTools />
      <SermonPicker />
    </>
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
      <Button
        variant="ghost"
        testId="editor-close"
        onClick={() => {
          void confirmDiscard().then((ok) => {
            if (!ok) return;
            closeFile();
            navigate("library");
          });
        }}
      >
        {t("app.editor.close")}
      </Button>
    </div>
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
