/**
 * `WaveformHost` — kontrakten S1b skrev ned for fase P, holdt.
 *
 *   1. **ÉN stabil `<canvas>`-ref**, pluss én for minimapet. Aldri remontert,
 *      aldri betinget pakket inn, aldri re-keyet. En canvas som remonteres
 *      mister konteksten sin, og en tegneløkke som holder på den gamle tegner
 *      videre i et element ingen ser — bølgeformen «fryser» uten at noe feiler.
 *   2. **`ResizeObserver` → `syncCanvasSize`**, ikke en `resize`-lytter på
 *      vinduet. Bølgeformen blir også smalere fordi noe ved siden av vokste,
 *      og det ser `resize` ingenting til. `canvas.width` skrives bare når
 *      tallet faktisk er et annet, fordi å skrive det NULLSTILLER lerretet.
 *   3. **`effect()` abonnerer tegne-scheduleren** på speilene, og scheduleren
 *      samler alt i ett rAF. En komponent som tegnet per signalskriving ville
 *      malt tre ganger i samme frame.
 *   4. **Unmount avbryter**: `cancelDraw`, `observer.disconnect()` og
 *      effektens egen dispose. En levende rAF etter at editoren er lukket
 *      koster et frame-budsjett opptaket trenger.
 *   5. **Dekoding og topputtrekk er UTENFOR** komponenten (`loader.ts`), nøklet
 *      på opptaket, så et bytte av steg ikke kaster arbeid.
 *
 * ⚠️ Ingen UA-forgrening noe sted her. WKWebView-en denne appen kjører i sender
 * en UA UTEN «Safari»-token, og det er nøyaktig faktumet bak SundayEdits 42×
 * regresjon: et bibliotek som sniffer etter Safari ser «ukjent motor» og tar
 * sin tregeste sti. Ytelsen her måles i `npm run tauri dev`, ikke i Chromium.
 *
 * ## Gullvinduet er DOM, ikke piksler
 *
 * Håndtakene er ekte knapper med `role="slider"`. En tegnet firkant kan ikke
 * fokuseres, kan ikke leses opp og kan ikke flyttes med piltastene — og en
 * frivillig som ikke bruker mus skal kunne justere hvor prekenen begynner.
 */

import { effect } from "@preact/signals";
import { useEffect, useRef } from "preact/hooks";
import type { JSX } from "preact";

import { t, tf } from "../i18n";
import {
  beginWindowHandleDrag,
  invalidateCanvasRect,
  isWindowPointerDrag,
  onCanvasPointerDown,
  onCanvasPointerLeave,
  onCanvasPointerMove,
  onCanvasWheel,
  onMinimapPointerDown,
} from "./canvas-input";
import { setSermonWindow } from "./cuts";
import { dragHandle, exactSpan, sermonWindow, timecode } from "./editor-core";
import {
  applied,
  cuts,
  dragWindow,
  duration,
  E,
  playheadSec,
  suggestion,
  viewport,
} from "./model";
import { spanLabel } from "./span";
import { centerOn } from "./viewport";
import styles from "./editor.module.css";
import { cancelDraw, scheduleDraw, syncCanvasSize } from "./waveform";

/** Hvor mange sekunder en piltast flytter et håndtak. Shift gir fem ganger. */
const NUDGE_SEC = 1;
/** PageUp/PageDown på et håndtak: et halvt minutt. */
const PAGE_SEC = 30;
const HANDLE_KEYS = new Set([
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
  "ArrowDown",
  "PageUp",
  "PageDown",
  "Home",
  "End",
]);

export function WaveformHost() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const minimapRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const minimap = minimapRef.current;
    if (!canvas || !minimap) return;
    E.canvas = canvas;
    E.minimap = minimap;

    const sync = (): void => {
      invalidateCanvasRect();
      syncBoth(canvas, minimap);
      scheduleDraw();
    };
    sync();

    const observer = new ResizeObserver(sync);
    observer.observe(canvas);
    observer.observe(minimap);

    // `passive: false` må settes eksplisitt: en hjul-lytter er passiv som
    // standard i moderne motorer, og en passiv lytter kan ikke kalle
    // `preventDefault()` — så siden ville rullet i stedet for å zoome.
    canvas.addEventListener("wheel", onCanvasWheel, { passive: false });
    document.addEventListener("scroll", invalidateCanvasRect, {
      capture: true,
      passive: true,
    });
    window.addEventListener("resize", invalidateCanvasRect);

    // Tegn på nytt når noe treet vet om har endret seg. Selve tegningen leser
    // `E`, aldri et signal — se toppen av `waveform.ts`.
    const dispose = effect(() => {
      void cuts.value;
      void playheadSec.value;
      void viewport.value;
      void duration.value;
      void suggestion.value;
      scheduleDraw();
    });

    return () => {
      dispose();
      cancelDraw();
      observer.disconnect();
      canvas.removeEventListener("wheel", onCanvasWheel);
      document.removeEventListener("scroll", invalidateCanvasRect, {
        capture: true,
      });
      window.removeEventListener("resize", invalidateCanvasRect);
      E.canvas = null;
      E.minimap = null;
    };
  }, []);

  return (
    <div class={styles.wave} data-testid="editor-wave">
      <canvas
        ref={canvasRef}
        // Bølgeformen er en tegning av lyd. Den tilgjengelige utgaven av det
        // den sier står i kortet over og i resultatlinja under — to stemmer om
        // det samme er verre enn én.
        aria-hidden="true"
        class={styles.canvas}
        data-testid="editor-canvas"
        onPointerDown={onCanvasPointerDown}
        onPointerMove={onCanvasPointerMove}
        onPointerLeave={onCanvasPointerLeave}
      />
      <KeepWindow />
      <canvas
        ref={minimapRef}
        aria-hidden="true"
        class={styles.minimap}
        data-testid="editor-minimap"
        onPointerDown={onMinimapPointerDown}
      />
    </div>
  );
}

/** Lerretets bakgrunnslager, for begge, i én omgang. */
function syncBoth(canvas: HTMLCanvasElement, minimap: HTMLCanvasElement): void {
  syncCanvasSize(canvas);
  syncCanvasSize(minimap);
}

// ── Gullvinduet ─────────────────────────────────────────────────────────────

/**
 * «Preken · 28 min», med et håndtak i hver ende.
 *
 * Det samme vinduet uansett hvilken side av «Behold bare prekenen» man står
 * på — `sermonWindow` i `editor-core` er den ene som avgjør hvilken av de to
 * representasjonene som gjelder.
 */
function KeepWindow() {
  // Et pågående drag vinner: det er det eneste som er sant mellom
  // `pointerdown` og `pointerup`.
  const window_ =
    dragWindow.value ??
    sermonWindow({
      cuts: cuts.value,
      duration: duration.value,
      suggestion: suggestion.value,
      applied: applied.value,
    });
  const view = viewport.value;
  const visible = view.end - view.start;
  if (!window_ || visible <= 0) return null;

  const left = ((window_.start - view.start) / visible) * 100;
  const right = ((window_.end - view.start) / visible) * 100;
  // Helt utenfor utsnittet: ingenting å tegne, og ingen håndtak å tabbe til.
  if (right <= 0 || left >= 100) return null;

  // SAMME tall som forslagskortet over bølgeformen. Avrundede minutter her og
  // sekundpresist der ville betydd «Preken · 4 min» rett under «… — 3 min
  // 30 s», altså to tall om det samme, samtidig, på samme skjerm.
  const span = spanLabel(exactSpan(window_.end - window_.start));

  return (
    <>
      <div
        class={styles.keep}
        data-testid="editor-keep"
        data-applied={applied.value ? "true" : "false"}
        style={{
          left: `${clampPercent(left)}%`,
          width: `${clampPercent(right) - clampPercent(left)}%`,
        }}
      >
        <span class={styles.keepLabel}>
          {tf("app.editor.keepLabel", {
            name: t("app.editor.sermon"),
            span,
          })}
        </span>
      </div>
      <Handle side="start" window={window_} at={left} />
      <Handle side="end" window={window_} at={right} />
    </>
  );
}

function Handle({
  side,
  window: window_,
  at,
}: {
  side: "start" | "end";
  window: { start: number; end: number };
  at: number;
}) {
  if (at < 0 || at > 100) return null;
  const total = duration.value;
  const value = side === "start" ? window_.start : window_.end;

  // Tastaturet går gjennom `dragWindow`, akkurat som musa: en holdt piltast
  // fyrer ~30 ganger i sekundet, og hver av dem som et eget angre-steg og en
  // egen utkastskriving fylte den 50 steg lange historikken på to sekunder
  // (audit 2026-11-11 🟠). Nå er ett trykk — eller ett hold — ETT steg,
  // registrert når tasten slippes eller fokus går.
  const moveTo = (sec: number): void => {
    const next = dragHandle(window_, side, sec, total);
    if (next.start === window_.start && next.end === window_.end) return;
    dragWindow.value = next;
    // Et håndtak som går ut av utsnittet forsvinner (se over), og fokuset
    // med det. Følg det i stedet.
    const at = side === "start" ? next.start : next.end;
    const view = viewport.peek();
    if (at < view.start || at > view.end) centerOn(at);
  };

  const flush = (): void => {
    const pending = dragWindow.peek();
    // Musa eier `dragWindow` mens den drar; den registrerer selv.
    if (!pending || isWindowPointerDrag()) return;
    dragWindow.value = null;
    setSermonWindow(pending, applied.value);
  };

  const onKeyDown = (
    event: JSX.TargetedKeyboardEvent<HTMLButtonElement>,
  ): void => {
    const step = event.shiftKey ? NUDGE_SEC * 5 : NUDGE_SEC;
    const target = (() => {
      switch (event.key) {
        case "ArrowLeft":
        case "ArrowDown":
          return value - step;
        case "ArrowRight":
        case "ArrowUp":
          return value + step;
        case "PageDown":
          return value - PAGE_SEC;
        case "PageUp":
          return value + PAGE_SEC;
        case "Home":
          return 0;
        case "End":
          return total;
        default:
          return null;
      }
    })();
    if (target === null) {
      // En annen tast (⌘Z, Tab, mellomrom …) midt i et hold: registrer det
      // som er flyttet FØRST. På macOS kommer ingen keyup for en pil som
      // slippes mens ⌘ er nede, og ⌘Z skal angre et ferdig steg — ikke
      // ignoreres mot et draft som aldri ble registrert.
      if (!HANDLE_KEYS.has(event.key)) flush();
      return;
    }
    event.preventDefault();
    moveTo(target);
  };

  const onKeyUp = (
    event: JSX.TargetedKeyboardEvent<HTMLButtonElement>,
  ): void => {
    if (HANDLE_KEYS.has(event.key)) flush();
  };

  return (
    <button
      type="button"
      role="slider"
      class={styles.handle}
      data-side={side}
      data-testid={`editor-handle-${side}`}
      style={{ left: `${at}%` }}
      aria-label={
        side === "start"
          ? t("app.editor.handleStart")
          : t("app.editor.handleEnd")
      }
      aria-valuemin={0}
      aria-valuemax={Math.round(total)}
      aria-valuenow={Math.round(value)}
      aria-valuetext={timecode(value)}
      onKeyDown={onKeyDown}
      onKeyUp={onKeyUp}
      onBlur={flush}
      onPointerDown={(event) =>
        beginWindowHandleDrag(side, event, applied.value)
      }
    />
  );
}

function clampPercent(value: number): number {
  return Math.max(0, Math.min(100, value));
}
