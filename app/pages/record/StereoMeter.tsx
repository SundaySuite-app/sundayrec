/**
 * StereoMeter — to LED-stolper, L og R («Én flate»-skissen).
 *
 * Tre former, samme data:
 *   `wide`      vannrett, med L/R-merker — hvilemåleren og lydarket.
 *   `vertical`  to tynne loddrette stolper — på kamerabildet.
 *   `mini`      to 30 px stolper — i kilde-brikka.
 *
 * ## Data, ikke dekor
 *
 * Skissen animerer et falskt dekke. Her er dekket det EKTE nivået:
 *   - `live="vu"`        den delte VU-strømmen (`acquireVuFeed`, `vu://levels`).
 *                        Paret som vises er det valgte (`pick`); mono speiler.
 *   - `live="recording"` motorens `recording://levels` (`peak_db_*`).
 * dBFS −60…0 avbildes til 0…100 % bredde (`levelFraction`); fargene står fast
 * under dekket: grønn til 72 %, gul til 88 %, så rød.
 *
 * Dekket settes IMPERATIVT på DOM-noden per pakke (30/s) og ikke gjennom
 * tilstand: en render per pakke ville rendret alt rundt måleren 30 ganger i
 * sekundet. Et kort fall (`createLevelSmoother`, release 80 ms) holder stolpen
 * lesbar; stigningen er umiddelbar. Ingen CSS-animasjon, så reduced-motion
 * ikke har noe å slå av.
 *
 * ## Vakten (ukjent bakgrunn)
 *
 * `off` = ingen strøm åpnes (ingen kilde valgt, eller det tas opp): stolpene
 * står tomme og sier det de vet. MONTERINGEN er også vakten, som for
 * `VuMeter`: ingen måler i treet, ingen `start_vu`.
 */

import { useEffect, useRef, useState } from "preact/hooks";

import { acquireVuFeed, type VuPick } from "@lib/audio/vu-feed";
import { createLevelSmoother } from "@lib/audio/smoothing";
import { VU_FLOOR_DB } from "@lib/audio/vu-feed-core";

import { levelFraction } from "../../audio/level-words";
import type { VuSource } from "../../ui/VuMeter/VuMeter";
import { recordingLevelsSource } from "./recording-levels";
import styles from "./StereoMeter.module.css";

export type StereoVariant = "wide" | "vertical" | "mini";

export interface StereoMeterProps {
  variant?: StereoVariant;
  /** Hvor nivåene kommer fra. */
  live?: "vu" | "recording";
  /** Enheten som måles (`live="vu"`). */
  deviceName?: string | null;
  /** Kanalvalget, lest per pakke (`live="vu"`). */
  pick?: () => VuPick;
  /** AVSLÅTT: ingen strøm, tomme stolper. */
  off?: boolean;
  /** Dekkets farge: hva stolpene står mot (`page` = `--bg`, `card` = `--raised`). */
  on?: "page" | "card";
  testId?: string;
}

const DEFAULT_PICK: VuPick = { mode: "stereo", chL: 0, chR: 1 };

/** Alle pakker som `VuSource` — den delte VU-strømmen pakket som kilde. */
function vuSource(
  deviceName: string | null | undefined,
  pick: () => VuPick,
): VuSource {
  return (emit) =>
    acquireVuFeed({
      deviceName,
      pick,
      onLevels: (l, r, raw) => {
        const p = pick();
        const peak = raw.peak_dbfs ?? [];
        const pl = Number.isFinite(peak[p.chL]) ? (peak[p.chL] as number) : l;
        const pr = Number.isFinite(peak[p.chR]) ? (peak[p.chR] as number) : r;
        emit({
          l,
          r,
          peakL: pl,
          peakR: pr,
          mono: p.mode !== "stereo",
        });
      },
    });
}

export function StereoMeter({
  variant = "wide",
  live = "vu",
  deviceName,
  pick,
  off = false,
  on = "page",
  testId,
}: StereoMeterProps) {
  const coverL = useRef<HTMLSpanElement | null>(null);
  const coverR = useRef<HTMLSpanElement | null>(null);
  const pickRef = useRef<() => VuPick>(pick ?? (() => DEFAULT_PICK));
  pickRef.current = pick ?? (() => DEFAULT_PICK);
  const [feed, setFeed] = useState<"idle" | "live" | "off">(
    off ? "off" : "idle",
  );
  const [mono, setMono] = useState(false);
  const vertical = variant === "vertical";

  useEffect(() => {
    const paint = (el: HTMLSpanElement | null, db: number): void => {
      if (!el) return;
      const empty = `${((1 - levelFraction(db)) * 100).toFixed(1)}%`;
      if (vertical) el.style.height = empty;
      else el.style.width = empty;
    };
    paint(coverL.current, VU_FLOOR_DB);
    paint(coverR.current, VU_FLOOR_DB);
    if (off) {
      setFeed("off");
      return;
    }
    setFeed("idle");
    const left = createLevelSmoother();
    const right = createLevelSmoother();
    let last = performance.now();
    let seen = false;
    let monoNow = false;
    const source =
      live === "recording"
        ? recordingLevelsSource
        : vuSource(deviceName, () => pickRef.current());
    const release = source((packet) => {
      const now = performance.now();
      const dt = now - last;
      last = now;
      paint(coverL.current, left.step(packet.l, dt));
      paint(coverR.current, right.step(packet.r, dt));
      if (!seen) {
        seen = true;
        setFeed("live");
      }
      const nextMono = packet.mono === true;
      if (nextMono !== monoNow) {
        monoNow = nextMono;
        setMono(nextMono);
      }
    });
    return () => {
      release();
    };
  }, [off, live, deviceName, vertical]);

  const cover = on === "card" ? styles.onCard : "";
  return (
    <span
      aria-hidden="true"
      data-testid={testId}
      data-feed={feed}
      data-mono={mono ? "true" : undefined}
      data-variant={variant}
      class={`${styles.meter} ${styles[variant]} ${cover}`}
    >
      <span class={styles.row}>
        {variant === "wide" ? <span class={styles.label}>L</span> : null}
        <span class={styles.track}>
          <span ref={coverL} class={styles.cover} />
        </span>
      </span>
      <span class={styles.row}>
        {variant === "wide" ? <span class={styles.label}>R</span> : null}
        <span class={styles.track}>
          <span ref={coverR} class={styles.cover} />
        </span>
      </span>
    </span>
  );
}
