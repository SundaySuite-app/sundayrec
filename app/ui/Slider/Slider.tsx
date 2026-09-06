/**
 * Slider — en `<input type="range">` med verdien lest ut ved siden av.
 *
 * To hendelser, med vilje forskjellige:
 *
 *   `onInput`  under draget — oppdaterer bare det tallet som vises,
 *   `onChange` når fingeren slippes — det er DA endringen committes.
 *
 * `planCommit('slider')` sier akkurat det samme (`events: ['change']`), og
 * grunnen står der: et drag som skrev ved hvert steg ville lagret førti ganger
 * for én bevegelse. Verdien man SER må likevel følge fingeren, ellers ser
 * kontrollen ødelagt ut — derav de to.
 *
 * `format` er inn og ikke ut: «−50 dB», «5 min» og «2» er tre forskjellige
 * enheter, og enheten er kallstedets, ikke bibliotekets.
 */

import type { JSX } from "preact";

import styles from "./Slider.module.css";

export interface SliderProps {
  value: number;
  min: number;
  max: number;
  step?: number;
  /** Under draget. */
  onInput?: (next: number) => void;
  /** Ved slipp — dette er commit-øyeblikket. */
  onChange: (next: number) => void;
  /** Verdien som tekst, med enhet. Utelates for ingen avlesning. */
  format?: (value: number) => string;
  disabled?: boolean;
  labelId?: string;
  describedBy?: string;
  testId?: string;
}

export function Slider({
  value,
  min,
  max,
  step = 1,
  onInput,
  onChange,
  format,
  disabled = false,
  labelId,
  describedBy,
  testId,
}: SliderProps) {
  const read = format ? format(value) : null;

  return (
    <span class={styles.wrap}>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        aria-labelledby={labelId}
        aria-describedby={describedBy}
        // Tallet alene («−50») sier ingenting; avlesningen med enhet gjør det.
        aria-valuetext={read ?? undefined}
        data-testid={testId}
        class={styles.range}
        onInput={(event: JSX.TargetedEvent<HTMLInputElement>) =>
          onInput?.(Number(event.currentTarget.value))
        }
        onChange={(event: JSX.TargetedEvent<HTMLInputElement>) =>
          onChange(Number(event.currentTarget.value))
        }
      />
      {read !== null ? (
        <span
          data-testid={testId ? `${testId}-value` : undefined}
          class={styles.value}
        >
          {read}
        </span>
      ) : null}
    </span>
  );
}
