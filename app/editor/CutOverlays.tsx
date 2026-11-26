/**
 * Det som ligger OPPÅ bølgeformen som DOM (E7): utklippede områder med
 * skravur og en rund «angre»-knapp, stiplede stillhetsforslag, og boblen over
 * en markering.
 *
 * Alt er ekte elementer og ikke tegnede piksler: knappene må kunne fokuseres,
 * leses opp og trykkes. Overleggene har `pointer-events: none` bortsett fra
 * knappene selv — draget, kantene og spillehodet lever fortsatt i canvaset
 * (`canvas-input.ts`) og skal ikke stjeles av en gjennomsiktig flate.
 *
 * Alle handlinger går gjennom de vanlige operasjonene (`deleteCut`, `addCut`,
 * `addCuts`), så angre, gjør om og utkastet virker som før.
 */

import { t } from "../i18n";
import { Icon } from "../ui/Icon/Icon";
import { deleteCut } from "./cuts";
import { exactSpan } from "./editor-core";
import { byTestId, focusAfterRender } from "./focus";
import { cuts, duration, selection, viewport } from "./model";
import { clearSelection, cutSelection } from "./selection";
import {
  acceptSuggestion,
  dismissSuggestion,
  suggestions,
} from "./silence-suggest";
import { spanLabel } from "./span";
import styles from "./editor.module.css";

/** Så nær filkanten (sekunder) regnes et kutt som hode eller hale. */
const EDGE_SEC = 0.01;

const RESTORE_SELECTOR = '[data-testid="editor-cut-restore"]';

/** Et tidsspenn → `left`/`width` i prosent av utsnittet, eller `null` helt utenfor. */
function place(
  start: number,
  end: number,
  view: { start: number; end: number },
): { left: number; width: number } | null {
  const visible = view.end - view.start;
  if (visible <= 0) return null;
  const left = ((start - view.start) / visible) * 100;
  const right = ((end - view.start) / visible) * 100;
  if (right <= 0 || left >= 100) return null;
  const l = Math.max(0, left);
  return { left: l, width: Math.min(100, right) - l };
}

/**
 * Utklippede områder midt i opptaket: skravert, med en rund knapp som legger
 * biten tilbake. Hodet og halen er IKKE med — de er gullhåndtakenes sak
 * (`KeepWindow`), og en «angre» der ville flyttet håndtaket uten at noen så
 * hva som skjedde.
 */
export function CutRegions() {
  const view = viewport.value;
  const total = duration.value;
  return (
    <>
      {cuts.value.map((cut, index) => {
        if (cut.start <= EDGE_SEC || cut.end >= total - EDGE_SEC) return null;
        const at = place(cut.start, cut.end, view);
        if (!at) return null;
        return (
          <div
            key={`${cut.start}:${cut.end}`}
            class={styles.cutRegion}
            data-testid="editor-cut-region"
            style={{ left: `${at.left}%`, width: `${at.width}%` }}
          >
            <button
              type="button"
              class={styles.roundBtn}
              data-testid="editor-cut-restore"
              aria-label={t("editor.deleteCut")}
              title={t("editor.deleteCut")}
              onClick={() => {
                deleteCut(index);
                focusAfterRender(
                  () =>
                    document.querySelectorAll(RESTORE_SELECTOR)[index] ?? null,
                  () =>
                    document.querySelectorAll(RESTORE_SELECTOR)[index - 1] ??
                    null,
                  byTestId('[data-testid="editor-silence"]'),
                );
              }}
            >
              <Icon name="restore" size={14} strokeWidth={2.2} />
            </button>
          </div>
        );
      })}
    </>
  );
}

/** Stiplede forslag. Ingenting klippes før «Godta» (eller «Godta alle») trykkes. */
export function SilenceRegions() {
  const view = viewport.value;
  return (
    <>
      {suggestions.value.map((s, index) => {
        const at = place(s.start, s.end, view);
        if (!at) return null;
        return (
          <div
            key={`${s.start}:${s.end}`}
            class={styles.suggestion}
            data-testid="editor-silence-region"
            style={{ left: `${at.left}%`, width: `${at.width}%` }}
          >
            <span class={styles.suggestionBtns}>
              <button
                type="button"
                class={`${styles.roundBtn} ${styles.roundGold}`}
                data-testid="editor-silence-accept"
                aria-label={t("app.editor.silenceAccept")}
                title={t("app.editor.silenceAccept")}
                onClick={() => {
                  acceptSuggestion(index);
                  focusAfterRender(
                    byTestId('[data-testid="editor-silence-accept"]'),
                    byTestId('[data-testid="editor-silence"]'),
                  );
                }}
              >
                <Icon name="check" size={14} strokeWidth={2.4} />
              </button>
              <button
                type="button"
                class={styles.roundBtn}
                data-testid="editor-silence-dismiss"
                aria-label={t("app.editor.silenceDismiss")}
                title={t("app.editor.silenceDismiss")}
                onClick={() => {
                  dismissSuggestion(index);
                  focusAfterRender(
                    byTestId('[data-testid="editor-silence-accept"]'),
                    byTestId('[data-testid="editor-silence"]'),
                  );
                }}
              >
                <Icon name="x" size={14} strokeWidth={2.2} />
              </button>
            </span>
          </div>
        );
      })}
    </>
  );
}

/**
 * Boblen over en markering: lengden, en gul «Klipp»-knapp og et kryss.
 *
 * Står utenfor bølgeformens `overflow: hidden`, midtstilt over markeringen og
 * klemt inn så den aldri havner utenfor kantene.
 */
export function SelectionBubble() {
  const sel = selection.value;
  const view = viewport.value;
  if (!sel) return null;
  const visible = view.end - view.start;
  if (visible <= 0) return null;
  const mid = ((sel.start + sel.end) / 2 - view.start) / visible;
  const center = Math.max(0, Math.min(1, mid)) * 100;
  return (
    <div
      class={styles.bubble}
      role="group"
      aria-label={t("app.editor.selectionAria")}
      data-testid="editor-selection"
      style={{ left: `clamp(8rem, ${center}%, calc(100% - 8rem))` }}
    >
      <span class={styles.bubbleLen} data-testid="editor-selection-length">
        {spanLabel(exactSpan(sel.end - sel.start))}
      </span>
      <button
        type="button"
        class={styles.cutBtn}
        data-testid="editor-selection-cut"
        title={t("app.editor.cutSelectionTitle")}
        onClick={cutSelection}
      >
        <Icon name="scissors" size={16} strokeWidth={2} />
        <span>{t("app.editor.cutSelection")}</span>
      </button>
      <button
        type="button"
        class={styles.bubbleClose}
        data-testid="editor-selection-cancel"
        aria-label={t("app.editor.cancelSelection")}
        title={t("app.editor.cancelSelection")}
        onClick={clearSelection}
      >
        <Icon name="x" size={16} strokeWidth={2} />
      </button>
    </div>
  );
}
