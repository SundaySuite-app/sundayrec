/**
 * Toggle — en `<button role="switch">`, og INGEN skjult `<input type=checkbox>`.
 *
 * Legacy-skallets brytere er en avkrysningsboks med `opacity: 0` og en
 * `<span>` som males til å ligne en bryter. Det gir tre feilkilder som alle
 * har slått til i denne appen: klikk som treffer boksen og ikke sporet
 * (bryteren «virker ikke»), en `checked` som DOM-en endrer før koden får se
 * det (og som `bindSetting` derfor må lese ut av hendelsen), og
 * `applyTranslations()` som stryker etiketten fordi den ligger i en `<label>`.
 *
 * `role="switch"` + `aria-checked` er den samme oppførselen uten noen av dem:
 * knappen kan bare klikkes ett sted, tilstanden er alltid den vi satte, og
 * Space/Enter kommer gratis fordi det ER en knapp.
 *
 * `aria-labelledby` kommer fra SettingRow — bryteren har ingen egen tekst, og
 * en bryter uten navn er en bryter en skjermleser leser som «på».
 */

import styles from "./Toggle.module.css";

export interface ToggleProps {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  /** Fra SettingRow. */
  labelId?: string;
  describedBy?: string;
  testId?: string;
}

export function Toggle({
  checked,
  onChange,
  disabled = false,
  labelId,
  describedBy,
  testId,
}: ToggleProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked ? "true" : "false"}
      aria-labelledby={labelId}
      aria-describedby={describedBy}
      aria-disabled={disabled ? "true" : undefined}
      data-testid={testId}
      class={`${styles.toggle} ${checked ? styles.on : ""} ${
        disabled ? styles.off : ""
      }`}
      onClick={() => {
        if (disabled) return;
        onChange(!checked);
      }}
    >
      <span aria-hidden="true" class={styles.knob} />
    </button>
  );
}
