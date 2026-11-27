/**
 * «Hold» — den røde stoppknappen i opptaksmodus. Må holdes inne i ett sekund;
 * en ring rundt fylles mens du holder, og slipper du tidlig, går ringen tilbake
 * og ingenting stopper.
 *
 *   Peker:     trykk ned = start, slipp / forlat knappen / avbrutt = avbryt.
 *   Tastatur:  hold Mellomrom eller Enter i ett sekund; slipp tidlig = avbryt.
 *              Tastegjentakelsen (`repeat`) teller ikke som et nytt trykk, og
 *              mistet fokus avbryter.
 *
 * ## Reduced motion
 *
 * Ringen fylles ellers med en CSS-overgang på ett sekund. Med redusert bevegelse
 * står overgangen av (base.css), og en ring som hoppet til full ved første trykk
 * ville sagt «ferdig» ett sekund for tidlig. Da fylles den i fire steg (hvert
 * kvarte sekund) — ingen animasjon, men ringen sier ærlig hvor langt holdet har
 * kommet. Selve holdet er like langt: `HOLD_TO_STOP_MS`.
 *
 * ## Skjermlesere
 *
 * VoiceOver og Narrator kan ikke HOLDE en knapp: de «klikker» den, og et
 * klikk uten pekertrykk (`event.detail === 0`) går til `onAssistiveClick` —
 * bekreftelsesdialogen i `stop.ts`. Et pekerklikk (`detail > 0`) gjør
 * fortsatt ingenting: holdet er hele sikringen mot uhell.
 *
 * Selve stoppet (vaktene, øktas identitet) bor i `stop.ts`; denne komponenten
 * eier bare «har hun holdt lenge nok?». `onBegin` kalles når holdet STARTER
 * (der økta leses) og kan avvise det ved å svare `false`.
 */

import { useEffect, useRef, useState } from "preact/hooks";

import { t } from "../../i18n";
import styles from "./HoldStop.module.css";
import { HOLD_TO_STOP_MS } from "./stop";

export interface HoldStopProps {
  /** Holdet er fullført. */
  onStop: () => void;
  /** Holdet begynner. `false` = avvis (motoren skriver ferdig, eller det går
   *  ikke noe opptak): ringen fyller seg da ikke. */
  onBegin?: () => boolean;
  /** Et klikk uten pekertrykk (skjermleser): spør i stedet for å holde. */
  onAssistiveClick?: () => void;
  /** Motoren skriver ferdig: knappen er av. */
  disabled?: boolean;
}

/** Ringens omkrets (r = 50) — `stroke-dasharray` og full `dashoffset`. */
const RING = 314;

const STEPS = 4;

/** Hvor langt utenfor knappen en holdt peker kan komme før holdet avbrytes. */
const LEAVE_SLACK_PX = 8;

function reducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

export function HoldStop({
  onStop,
  onBegin,
  onAssistiveClick,
  disabled = false,
}: HoldStopProps) {
  const [holding, setHolding] = useState(false);
  // 0…1: bare brukt i redusert bevegelse; ellers går ringen mot 1 på egen hånd.
  const [step, setStep] = useState(0);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);
  const heldKey = useRef<string | null>(null);
  const onStopRef = useRef(onStop);
  onStopRef.current = onStop;
  const onBeginRef = useRef(onBegin);
  onBeginRef.current = onBegin;
  const onClickRef = useRef(onAssistiveClick);
  onClickRef.current = onAssistiveClick;
  const button = useRef<HTMLButtonElement | null>(null);

  function clearTimers(): void {
    for (const id of timers.current) clearTimeout(id);
    timers.current = [];
  }

  function cancel(): void {
    clearTimers();
    heldKey.current = null;
    setHolding(false);
    setStep(0);
  }

  function begin(): void {
    if (disabled || holding || timers.current.length > 0) return;
    if (onBeginRef.current && onBeginRef.current() === false) {
      heldKey.current = null;
      return;
    }
    setHolding(true);
    if (reducedMotion()) {
      for (let i = 1; i < STEPS; i++) {
        timers.current.push(
          setTimeout(() => setStep(i / STEPS), (HOLD_TO_STOP_MS * i) / STEPS),
        );
      }
    }
    timers.current.push(
      setTimeout(() => {
        // Holdet er fullført: knappen er ferdig brukt til dette trykket.
        clearTimers();
        heldKey.current = null;
        setHolding(false);
        setStep(0);
        onStopRef.current();
      }, HOLD_TO_STOP_MS),
    );
  }

  // Knappen forsvinner (opptaket tok slutt): ingen timer skal fyre etterpå.
  useEffect(() => clearTimers, []);
  useEffect(() => {
    if (disabled) cancel();
  }, [disabled]);

  // Overlegget er hele skjermen og knappen er det eneste å gjøre: ta imot
  // fokus, slik at et opptak startet med tastaturet kan stoppes med det.
  useEffect(() => {
    button.current?.focus({ preventScroll: true });
  }, []);

  const progress = reducedMotion() ? step : holding ? 1 : 0;

  return (
    <button
      ref={button}
      type="button"
      data-testid="overlay-stop"
      data-holding={holding ? "true" : undefined}
      aria-label={t("app.rec.holdAria")}
      title={t("app.rec.holdTitle")}
      aria-disabled={disabled ? "true" : undefined}
      class={styles.button}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        try {
          e.currentTarget.setPointerCapture(e.pointerId);
        } catch {
          /* en peker som ikke kan fanges: leave/cancel dekker resten */
        }
        begin();
      }}
      onPointerMove={(e) => {
        // Pekeren er fanget (touch fanger selv), så `pointerleave` kommer ikke
        // mens fingeren er nede: «forlot knappen» må sjekkes mot rammen.
        if (!holding) return;
        const r = e.currentTarget.getBoundingClientRect();
        const dx = e.clientX - (r.left + r.width / 2);
        const dy = e.clientY - (r.top + r.height / 2);
        if (Math.hypot(dx, dy) > r.width / 2 + LEAVE_SLACK_PX) cancel();
      }}
      onPointerUp={cancel}
      onPointerLeave={cancel}
      onPointerCancel={cancel}
      onLostPointerCapture={cancel}
      onBlur={cancel}
      onKeyDown={(e) => {
        if (e.key !== " " && e.key !== "Enter") return;
        // Ikke rull siden, og ikke la nettleseren «klikke» knappen.
        e.preventDefault();
        if (e.repeat || heldKey.current !== null) return;
        heldKey.current = e.key;
        begin();
      }}
      onKeyUp={(e) => {
        if (e.key === " " || e.key === "Enter") {
          // Mellomrom «klikker» først ved slipp: uten dette ville et hold
          // som slutter på tastaturet ende i dialogen.
          e.preventDefault();
        }
        if (e.key === heldKey.current) cancel();
      }}
      onClick={(e) => {
        // Pekerklikk (detail > 0) og tastatur (stoppet over) er holdets
        // saker. Bare et syntetisk klikk fra hjelpeteknologi har detail 0.
        if (e.detail !== 0 || disabled) return;
        onClickRef.current?.();
      }}
      onContextMenu={(e) => e.preventDefault()}
    >
      <svg
        viewBox="0 0 112 112"
        width="112"
        height="112"
        aria-hidden="true"
        class={styles.ring}
      >
        <circle cx="56" cy="56" r="50" class={styles.track} />
        <circle
          cx="56"
          cy="56"
          r="50"
          class={styles.fill}
          data-holding={holding ? "true" : "false"}
          style={{
            strokeDasharray: RING,
            strokeDashoffset: RING * (1 - progress),
          }}
        />
      </svg>
      <span aria-hidden="true" class={styles.square} />
    </button>
  );
}
