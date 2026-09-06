/**
 * Select — en ekte `<select>`.
 *
 * Ingen egenbygd nedtrekksmeny. En egenbygd får aldri OS-ets egen liste,
 * tastaturnavigasjonen, «skriv første bokstav»-hoppet eller den innebygde
 * rullingen på en liste med 32 kanaler, og alt det måtte bygges på nytt for å
 * ende opp litt dårligere. Den ene tingen vi styrer er utseendet på selve
 * boksen.
 *
 * `options` er `{ value, label }` og ikke barn: en `<option>` med tekst
 * skrevet rett inn i JSX ville vært hardkodet prosa, og gaten
 * (`check-i18n-hardcoded-tsx.mjs`) skal si fra om det. Etikettene kommer fra
 * katalogen på kallstedet.
 */

import type { JSX } from "preact";

import styles from "./Select.module.css";

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

export interface SelectProps {
  value: string;
  options: readonly SelectOption[];
  onChange: (next: string) => void;
  disabled?: boolean;
  labelId?: string;
  describedBy?: string;
  testId?: string;
}

export function Select({
  value,
  options,
  onChange,
  disabled = false,
  labelId,
  describedBy,
  testId,
}: SelectProps) {
  return (
    <select
      value={value}
      disabled={disabled}
      aria-labelledby={labelId}
      aria-describedby={describedBy}
      data-testid={testId}
      class={styles.select}
      onChange={(event: JSX.TargetedEvent<HTMLSelectElement>) =>
        onChange(event.currentTarget.value)
      }
    >
      {options.map((option) => (
        <option
          key={option.value}
          value={option.value}
          disabled={option.disabled}
        >
          {option.label}
        </option>
      ))}
    </select>
  );
}
