/**
 * TextArea — tekstfeltet over flere linjer, i samme drakt som `TextField`.
 *
 * Finnes for én ting ennå: beskrivelsen i eksportens «Innhold», som er
 * fritekst med linjeskift (bibeltekst, en fast linje om menigheten). Et
 * `<input>` ville spist linjeskiftene uten å si fra.
 *
 * `onCommit` fyrer på blur og ALDRI på Enter — i et felt over flere linjer er
 * Enter et linjeskift, ikke «ferdig». I eksporten er den ubrukt (verdien bor i
 * et signal til eksporten sender den); i Oppsett er den `useSetting`s «nå».
 */

import type { JSX } from "preact";

import styles from "./TextArea.module.css";

export interface TextAreaProps {
  value: string;
  onInput: (next: string) => void;
  /** Blur — aldri Enter. */
  onCommit?: () => void;
  /** MÅ komme fra katalogen — gaten sjekker `placeholder` som prosa. */
  placeholder?: string;
  rows?: number;
  disabled?: boolean;
  labelId?: string;
  describedBy?: string;
  testId?: string;
}

export function TextArea({
  value,
  onInput,
  onCommit,
  placeholder,
  rows = 3,
  disabled = false,
  labelId,
  describedBy,
  testId,
}: TextAreaProps) {
  return (
    <textarea
      value={value}
      rows={rows}
      placeholder={placeholder}
      disabled={disabled}
      aria-labelledby={labelId}
      aria-describedby={describedBy}
      data-testid={testId}
      class={styles.input}
      onInput={(event: JSX.TargetedEvent<HTMLTextAreaElement>) =>
        onInput(event.currentTarget.value)
      }
      onBlur={() => onCommit?.()}
    />
  );
}
