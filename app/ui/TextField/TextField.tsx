/**
 * TextField — ett tekstfelt, og skillet mellom «skriver» og «ferdig».
 *
 * `onInput` fyrer på hvert tastetrykk, `onCommit` på blur og Enter. Begge
 * finnes fordi `useSetting` trenger begge: et felt med etterslep committer på
 * `input` (ellers ville en verdi som skrives og forlates uten blur aldri bli
 * lagret — se `planCommit` i bind-setting-core), mens blur og Enter er «nå,
 * med en gang».
 *
 * Feilmeldingen står ikke her. Den eies av SettingRow, som setter
 * `aria-describedby` og maler linjen under kontrollen — ett sted per rad, ikke
 * ett per felttype. `invalid` er bare den røde kanten.
 */

import type { JSX } from "preact";

import styles from "./TextField.module.css";

export interface TextFieldProps {
  value: string;
  onInput: (next: string) => void;
  /** Blur og Enter. */
  onCommit?: () => void;
  /** MÅ komme fra katalogen — gaten sjekker `placeholder` som prosa. */
  placeholder?: string;
  /**
   * `password` (P1b) er SMTP-passordet, og bare det. Feltet leses aldri
   * tilbake: hemmeligheten bor i OS-nøkkelringen og krysser aldri inn i
   * webviewet igjen, så verdien her er alltid enten tom eller noe brukeren
   * nettopp skrev.
   */
  type?: "text" | "email" | "number" | "password";
  inputMode?: JSX.HTMLAttributes<HTMLInputElement>["inputMode"];
  disabled?: boolean;
  /** Rød kant. Teksten står i SettingRows feillinje. */
  invalid?: boolean;
  labelId?: string;
  describedBy?: string;
  testId?: string;
}

export function TextField({
  value,
  onInput,
  onCommit,
  placeholder,
  type = "text",
  inputMode,
  disabled = false,
  invalid = false,
  labelId,
  describedBy,
  testId,
}: TextFieldProps) {
  return (
    <input
      type={type}
      inputMode={inputMode}
      value={value}
      placeholder={placeholder}
      disabled={disabled}
      aria-labelledby={labelId}
      aria-describedby={describedBy}
      aria-invalid={invalid ? "true" : undefined}
      data-testid={testId}
      class={`${styles.input} ${invalid ? styles.invalid : ""}`}
      onInput={(event: JSX.TargetedEvent<HTMLInputElement>) =>
        onInput(event.currentTarget.value)
      }
      onBlur={() => onCommit?.()}
      onKeyDown={(event: JSX.TargetedKeyboardEvent<HTMLInputElement>) => {
        if (event.key !== "Enter") return;
        // Ikke `preventDefault` på et felt uten skjema rundt: Enter gjør
        // ingenting av seg selv her, og å stoppe den ville også stoppet
        // OS-ets egne tastatursnarveier i et framtidig skjema.
        onCommit?.();
      }}
    />
  );
}
