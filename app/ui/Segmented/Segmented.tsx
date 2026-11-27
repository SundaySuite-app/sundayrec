/**
 * Segmented — «Norsk | English», «Stabil | Beta»: to–tre valg i en pille,
 * ett av dem valgt.
 *
 * `role="radiogroup"` med `role="radio"`-knapper og `aria-checked`: det er
 * nøyaktig det en skjermleser skal høre («Norsk, valgt, 1 av 2»). Piltastene
 * flytter valget som i enhver radiogruppe, og bare det valgte segmentet er i
 * tabrekkefølgen (roving tabindex).
 *
 * Aldri en nedtrekksliste for to valg: listen skjuler det ene valget bak et
 * trykk, og det er hele poenget med tannhjulet at begge står der.
 */

import { useRef } from "preact/hooks";

import styles from "./Segmented.module.css";

export interface SegmentedOption {
  value: string;
  label: string;
  testId?: string;
}

export interface SegmentedProps {
  value: string;
  options: readonly SegmentedOption[];
  onChange: (next: string) => void;
  /** Navnet gruppen leses opp med. */
  label: string;
  disabled?: boolean;
  testId?: string;
}

export function Segmented({
  value,
  options,
  onChange,
  label,
  disabled = false,
  testId,
}: SegmentedProps) {
  const group = useRef<HTMLSpanElement | null>(null);
  const selected = options.some((o) => o.value === value)
    ? value
    : options[0]?.value;

  function onKeyDown(event: KeyboardEvent): void {
    const dir =
      event.key === "ArrowRight" || event.key === "ArrowDown"
        ? 1
        : event.key === "ArrowLeft" || event.key === "ArrowUp"
          ? -1
          : 0;
    if (dir === 0 || disabled) return;
    event.preventDefault();
    const at = options.findIndex((o) => o.value === selected);
    const next = options[(at + dir + options.length) % options.length];
    if (!next) return;
    onChange(next.value);
    // Fokus følger valget, men først etter at det er tegnet.
    queueMicrotask(() =>
      group.current
        ?.querySelector<HTMLElement>(`[data-value="${next.value}"]`)
        ?.focus(),
    );
  }

  return (
    <span
      ref={group}
      role="radiogroup"
      aria-label={label}
      data-testid={testId}
      class={styles.group}
      onKeyDown={onKeyDown}
    >
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={o.value === selected}
          aria-disabled={disabled ? "true" : undefined}
          tabIndex={o.value === selected ? 0 : -1}
          data-value={o.value}
          data-testid={o.testId}
          class={`${styles.segment} ${o.value === selected ? styles.on : ""}`}
          onClick={() => {
            if (!disabled && o.value !== selected) onChange(o.value);
          }}
        >
          {o.label}
        </button>
      ))}
    </span>
  );
}
