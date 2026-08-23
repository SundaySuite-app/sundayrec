/**
 * VuMeter — «hører vi lyd?», besvart uten et eneste tall.
 *
 * ## Én canvas-ref, og tegningen skjer utenfor render
 *
 * Måleren får ~30 pakker i sekundet. Å la hver pakke bli en `setState` ville
 * betydd 30 re-render i sekundet av alt som står rundt den, og da flytter
 * jank-en seg til resten av skjermen. Så: pakkene skrives inn i en `ref`, og
 * en rAF-løkke tegner. Det eneste som utløser en re-render er at ORDET endrer
 * seg («Vi hører ingenting» → «Vi hører lyd»), som skjer noen ganger i minuttet.
 *
 * Canvas-elementet er én stabil ref som aldri byttes ut. En canvas som
 * remonteres mister konteksten sin, og en tegneløkke som holder på en gammel
 * kontekst tegner videre i et element ingen ser — måleren «fryser» uten at noe
 * feiler.
 *
 * ## Fyllet er relativt til HELE stolpen
 *
 * Fargebåndet ligger fast: grønt fra 0 til 72 %, gult til 90 %, rødt over —
 * regnet av stolpens FULLE bredde. Fyllet klipper det båndet ved nivået.
 * Alternativet (en gradient over selve fyllet) ser nesten likt ut og er feil:
 * da ville et lavt nivå vist rødt i sin egen høyre ende, altså «for høyt» ved
 * −40 dB. Canvasens første utkast gjorde nettopp den feilen.
 *
 * ## Ingen tall på nivå 1
 *
 * Canvasens sett 0. `showNumbers` finnes for Avansert og opptaksoverlegget,
 * der den som vil ha dB-tall skal få dem.
 *
 * ## Én eier av mikrofonen
 *
 * `acquireVuFeed` er den delte bakenden-strømmen: Rust eier enheten, denne
 * prosessen LYTTER. Måleren åpner aldri sin egen `getUserMedia` — det er
 * feilklassen bak Qu-5-hendelsen 2026-07-31, der en webview som hadde åpnet
 * enheten holdt den låst til 2 kanaler mens opptaket ville hatt 32.
 * `release()` ved unmount er derfor ikke ryddighet, det er hele kontrakten.
 */

import { useEffect, useRef, useState } from "preact/hooks";

import {
  acquireVuFeed,
  type VuFeedState,
  type VuPick,
} from "@lib/audio/vu-feed";
import { createLevelSmoother } from "@lib/audio/smoothing";
import { pickLR, VU_FLOOR_DB } from "@lib/audio/vu-feed-core";
import type { VuLevels } from "@lib/../bindings/VuLevels";

import {
  levelFraction,
  levelWordFor,
  type LevelWord,
} from "../../audio/level-words";
import { tDyn } from "../../i18n";
import { StatusDot } from "../StatusDot/StatusDot";
import styles from "./VuMeter.module.css";

/**
 * Enheten på tallene under Avansert. En konstant, ikke tekst i treet: «dBFS»
 * er en SI-lignende enhet som er lik på alle sju språk, men prosa-gaten kan
 * ikke vite det og skal ikke måtte gjette.
 */
const UNIT_DBFS = "dBFS";

/** Fargebåndets grenser, som andel av stolpens fulle bredde. */
const AMBER_AT = 0.72;
const RED_AT = 0.9;
/** Kanal 1 og 2 i stereo — det stolpene viser når ingen sa noe annet. */
const DEFAULT_PICK: VuPick = { mode: "stereo", chL: 0, chR: 1 };

/** To stolper (venstre og høyre) med litt luft mellom. */
const BAR_H = 14;
const BAR_GAP = 8;

export interface VuMeterProps {
  /** Enheten som skal måles. `undefined` = «hva som enn kjører». */
  deviceName?: string | null;
  /**
   * Hvilke av enhetens native kanaler de to stolpene viser. Utelatt = kanal
   * 1 og 2 i stereo.
   *
   * En THUNK og ikke en verdi: valget leses per pakke, så et nytt kanalpar
   * slår inn på neste frame uten at strømmen startes på nytt — og en
   * enhetsåpning til er nøyaktig det ingen meter i denne appen skal be om.
   */
  pick?: () => VuPick;
  /** dB-tall ved siden av stolpene. Av på nivå 1 — se toppen av fila. */
  showNumbers?: boolean;
  testId?: string;
}

interface Levels {
  l: number;
  r: number;
  peakL: number;
  peakR: number;
}

export function VuMeter({
  deviceName,
  pick,
  showNumbers = false,
  testId,
}: VuMeterProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const levels = useRef<Levels>({
    l: VU_FLOOR_DB,
    r: VU_FLOOR_DB,
    peakL: VU_FLOOR_DB,
    peakR: VU_FLOOR_DB,
  });
  const [word, setWord] = useState<LevelWord>("nothing");
  const [feedState, setFeedState] = useState<VuFeedState>("idle");
  const wordRef = useRef<LevelWord>("nothing");
  const [readout, setReadout] = useState<{ l: number; r: number } | null>(null);
  // Kanalvalget leses per pakke gjennom en ref, så et nytt par ikke river opp
  // abonnementet — og dermed ikke starter enheten på nytt.
  const pickRef = useRef<(() => VuPick) | undefined>(pick);
  pickRef.current = pick;

  // ── Strømmen ──────────────────────────────────────────────────────────────
  useEffect(() => {
    const release = acquireVuFeed({
      deviceName,
      pick: () => pickRef.current?.() ?? DEFAULT_PICK,
      onLevels: (l, r, raw: VuLevels) => {
        const chosen = pickRef.current?.() ?? DEFAULT_PICK;
        const peak = pickLR(raw.peak_dbfs, chosen.mode, chosen.chL, chosen.chR);
        levels.current = { l, r, peakL: peak.l, peakR: peak.r };
        // Bare ORDET utløser en render, ikke pakken.
        const next = levelWordFor(peak.l, peak.r);
        if (next !== wordRef.current) {
          wordRef.current = next;
          setWord(next);
        }
      },
      onState: (state) => setFeedState(state),
    });
    return release;
  }, [deviceName]);

  // ── Tegningen ─────────────────────────────────────────────────────────────
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    // Utjevningen er husets ene bevegelseslov (`@lib/audio/smoothing`):
    // øyeblikkelig oppover, dempet nedover, og tidsbasert — så 60 → 30 fps
    // endrer hvor ofte vi tegner og ingenting annet ved hvordan nålen faller.
    const smoothL = createLevelSmoother({ initial: VU_FLOOR_DB });
    const smoothR = createLevelSmoother({ initial: VU_FLOOR_DB });

    let raf = 0;
    let last = performance.now();
    let alive = true;

    const style = getComputedStyle(canvas);
    const colour = (name: string): string =>
      style.getPropertyValue(name).trim();

    const sizeToBox = (): void => {
      const dpr = window.devicePixelRatio || 1;
      const width = canvas.clientWidth;
      const height = canvas.clientHeight;
      if (width === 0 || height === 0) return;
      const w = Math.round(width * dpr);
      const h = Math.round(height * dpr);
      // Å skrive `width` nullstiller canvasen — bare når den faktisk endret seg.
      if (canvas.width !== w) canvas.width = w;
      if (canvas.height !== h) canvas.height = h;
    };

    const paintBar = (y: number, h: number, w: number, db: number): void => {
      const radius = h / 2;
      // Sporet.
      ctx.fillStyle = colour("--raised");
      ctx.beginPath();
      ctx.roundRect(0, y, w, h, radius);
      ctx.fill();

      const fraction = levelFraction(db);
      if (fraction <= 0) return;

      // Båndet males i FULL bredde og klippes ved nivået — se toppen av fila.
      ctx.save();
      ctx.beginPath();
      ctx.roundRect(0, y, w, h, radius);
      ctx.clip();
      ctx.beginPath();
      ctx.rect(0, y, w * fraction, h);
      ctx.clip();

      ctx.fillStyle = colour("--good");
      ctx.fillRect(0, y, w * AMBER_AT, h);
      ctx.fillStyle = colour("--warn");
      ctx.fillRect(w * AMBER_AT, y, w * (RED_AT - AMBER_AT), h);
      ctx.fillStyle = colour("--rec");
      ctx.fillRect(w * RED_AT, y, w * (1 - RED_AT), h);
      ctx.restore();
    };

    const frame = (now: number): void => {
      if (!alive) return;
      const dt = now - last;
      last = now;
      sizeToBox();

      const dpr = window.devicePixelRatio || 1;
      const w = canvas.width;
      const barH = BAR_H * dpr;
      const gap = BAR_GAP * dpr;

      ctx.clearRect(0, 0, canvas.width, canvas.height);
      const l = smoothL.step(levels.current.l, dt);
      const r = smoothR.step(levels.current.r, dt);
      paintBar(0, barH, w, l);
      paintBar(barH + gap, barH, w, r);

      raf = requestAnimationFrame(frame);
    };
    raf = requestAnimationFrame(frame);

    // Bredden endres når vinduet gjør det. `ResizeObserver` og ikke en
    // `resize`-lytter: stolpene kan også bli smalere fordi noe ved siden av
    // vokste, og det ser `resize` ingenting til.
    const observer = new ResizeObserver(sizeToBox);
    observer.observe(canvas);

    return () => {
      alive = false;
      cancelAnimationFrame(raf);
      observer.disconnect();
    };
  }, []);

  // Tallene, når noen har bedt om dem. Ett hakk i sekundet er nok: et dB-tall
  // som oppdateres 30 ganger i sekundet er uleselig uansett.
  useEffect(() => {
    if (!showNumbers) return;
    const tick = setInterval(
      () => setReadout({ l: levels.current.l, r: levels.current.r }),
      1000,
    );
    return () => clearInterval(tick);
  }, [showNumbers]);

  const dot = word === "hear" ? "listen" : word === "loud" ? "warn" : "neutral";

  return (
    <div
      data-testid={testId}
      data-word={word}
      data-feed={feedState}
      class={styles.vu}
    >
      <div class={styles.head}>
        <StatusDot tone={dot} />
        <b
          data-testid={testId ? `${testId}-word` : undefined}
          class={styles.word}
        >
          {tDyn("app.vu", word)}
        </b>
        {showNumbers && readout ? (
          <span
            data-testid={testId ? `${testId}-numbers` : undefined}
            class={styles.numbers}
          >
            {`${Math.round(readout.l)} / ${Math.round(readout.r)} ${UNIT_DBFS}`}
          </span>
        ) : null}
      </div>
      <canvas
        ref={canvasRef}
        // Måleren er en LEVENDE avlesning; ordet over er den tilgjengelige
        // utgaven av den, og det er allerede i treet. To stemmer som sier det
        // samme er verre enn én.
        aria-hidden="true"
        data-testid={testId ? `${testId}-canvas` : undefined}
        class={styles.canvas}
      />
    </div>
  );
}
