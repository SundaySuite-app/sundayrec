/**
 * RadioCards — et valg der hvert alternativ får si hva det betyr.
 *
 * «God» og «Best» sier ingenting alene; «MP3. Passer for nett og deling. Ca.
 * 1 GB per 10 timer.» svarer på spørsmålet en frivillig faktisk har. Derfor
 * har hvert alternativ en beskrivelse, og derfor er dette kort og ikke en
 * `<select>` — en nedtrekksmeny kan ikke vise begrunnelsen før man har valgt.
 *
 * `recommended` er én brikke, på ett alternativ. To anbefalinger er ingen.
 *
 * ## Tilgjengelighet
 *
 * `role="radiogroup"` med ekte `<input type="radio">` under: piltastene,
 * gruppens tabstopp og «bare én i gruppen kan være valgt» kommer fra
 * nettleseren. Kortet er en `<label>` rundt inputen, så hele flaten er
 * klikkflate uten at vi må syntetisere noe.
 */

import type { JSX } from "preact";
import { useId } from "preact/hooks";

import { Chip } from "../Chip/Chip";
import { t } from "../../i18n";
import styles from "./RadioCards.module.css";

export interface RadioOption {
  value: string;
  title: string;
  description?: string;
  /** Brikken «Anbefalt». Sett den på HØYST ett alternativ. */
  recommended?: boolean;
  disabled?: boolean;
}

export interface RadioCardsProps {
  value: string;
  options: readonly RadioOption[];
  onChange: (next: string) => void;
  disabled?: boolean;
  /** Gruppens navn for skjermlesere (fra SettingRow, eller en egen tittel). */
  labelId?: string;
  describedBy?: string;
  /** To i bredden. Standard er én kolonne. */
  columns?: 1 | 2;
  testId?: string;
}

export function RadioCards({
  value,
  options,
  onChange,
  disabled = false,
  labelId,
  describedBy,
  columns = 1,
  testId,
}: RadioCardsProps) {
  const name = useId();

  return (
    <div
      role="radiogroup"
      aria-labelledby={labelId}
      aria-describedby={describedBy}
      data-testid={testId}
      class={`${styles.group} ${columns === 2 ? styles.two : ""}`}
    >
      {options.map((option) => {
        const selected = option.value === value;
        const off = disabled || option.disabled === true;
        return (
          <label
            key={option.value}
            data-testid={testId ? `${testId}-row-${option.value}` : undefined}
            data-selected={selected ? "true" : undefined}
            class={`${styles.card} ${selected ? styles.selected : ""} ${
              off ? styles.off : ""
            }`}
          >
            <input
              type="radio"
              name={name}
              value={option.value}
              checked={selected}
              disabled={off}
              class={styles.input}
              onChange={(event: JSX.TargetedEvent<HTMLInputElement>) => {
                if (event.currentTarget.checked) onChange(option.value);
              }}
            />
            <span aria-hidden="true" class={styles.mark} />
            <span class={styles.body}>
              <span class={styles.titleLine}>
                <span class={styles.title}>{option.title}</span>
                {option.recommended ? (
                  <Chip tone="gold">{t("app.common.recommended")}</Chip>
                ) : null}
              </span>
              {option.description ? (
                <span class={styles.description}>{option.description}</span>
              ) : null}
            </span>
          </label>
        );
      })}
    </div>
  );
}
