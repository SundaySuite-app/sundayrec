/**
 * «Analyserer …» og «Kunne ikke åpne» — de to tilstandene en åpning kan stå i,
 * på begge sidene som venter på den.
 *
 * De bodde i `EditorPage.tsx` fram til D3, fordi REDIGERING var det eneste
 * stedet en fil ble åpnet. Etter D3 er EKSPORTERING en egen destinasjon som
 * åpner filer selv («Gjør klar» på sist-redigert-kortet), og den venter på
 * nøyaktig det samme: samme faser, samme fremdriftskanal, samme feil.
 *
 * En kopi ville vært to steder som sier hver sin ting om den samme lastingen —
 * og den ene av dem ville sluttet å nevne fasen første gang noen la til en.
 * Så: ÉN kilde, to montører. Testid-ene er uendret (`editor-loading`,
 * `editor-loading-text`, `editor-loading-progress`, `editor-load-error`,
 * `editor-load-error-open`, `editor-load-error-close`) — de er kontrakter mot
 * e2e-laget, og en flytting er ingen grunn til å brekke dem.
 */

import { t, tDyn } from "../i18n";
import { Banner } from "../ui/Banner/Banner";
import { Button } from "../ui/Button/Button";
import { ProgressBar } from "../ui/ProgressBar/ProgressBar";
import { closeFile, pickAndOpen } from "./loader";
import { loadPhase, loadProgress } from "./model";
import styles from "./editor.module.css";

export function Loading() {
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

export function LoadFailed() {
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
