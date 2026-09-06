/**
 * SettingRow — én innstilling, og de fire faste plassene rundt den.
 *
 *     ┌─────────────────────────────────────────────┬──────────────┐
 *     │ Etikett          [kvittering]               │  kontrollen  │
 *     │ Forklaring i én setning                     │              │
 *     │                                             │  feillinje   │
 *     └─────────────────────────────────────────────┴──────────────┘
 *
 * Plassene er faste fordi de er de samme overalt: legacy-skallet har
 * «Lagret ✓» tre forskjellige steder (over kontrollen, under den, og som en
 * toast), og en frivillig kan ikke lære hvor hun skal se når svaret flytter
 * seg mellom skjermene.
 *
 * ## Kvitteringen står ved ETIKETTEN, feilen under KONTROLLEN
 *
 * Ikke smakssak. Kvitteringen svarer på «ble dette lagret?», som handler om
 * innstillingen — den hører til navnet. Feilen svarer på «hva er galt med det
 * jeg skrev?», som handler om verdien — den hører til feltet, og må stå der
 * øyet allerede er.
 *
 * ## Hvem eier id-ene
 *
 * Raden gjør. En kontroll som skal ha `aria-labelledby` (bryteren er en
 * `<button>`, ikke en `<input>` — `<label for>` virker ikke på den) og
 * `aria-describedby` får id-ene inn gjennom barnefunksjonen:
 *
 *     <SettingRow label={…}>{(ids) => <Toggle {...ids} … />}</SettingRow>
 *
 * Vanlige barn er også lov, for de kontrollene som merker seg selv.
 */

import type { ComponentChildren } from "preact";
import { useId } from "preact/hooks";

import { Receipt } from "../Receipt/Receipt";
import type { Receipt as ReceiptState } from "../../settings/use-setting-core";
import styles from "./SettingRow.module.css";

/** Id-ene kontrollen skal peke på. */
export interface RowIds {
  /** For `aria-labelledby`. */
  labelId: string;
  /** For `aria-describedby` — forklaringen og/eller feilen. */
  describedBy: string | undefined;
}

export interface SettingRowProps {
  label: string;
  description?: string;
  /** Kvitteringen fra `useSetting`. */
  receipt?: ReceiptState;
  /** Valideringsfeilen fra `useSetting`. */
  error?: string | null;
  /** Grået ut: en Gate over raden, eller en forutsetning som mangler. */
  disabled?: boolean;
  children: ComponentChildren | ((ids: RowIds) => ComponentChildren);
  testId?: string;
}

export function SettingRow({
  label,
  description,
  receipt = "idle",
  error = null,
  disabled = false,
  children,
  testId,
}: SettingRowProps) {
  const base = useId();
  const labelId = `${base}-label`;
  const descId = `${base}-desc`;
  const errorId = `${base}-error`;
  const describedBy =
    [description ? descId : null, error ? errorId : null]
      .filter(Boolean)
      .join(" ") || undefined;

  const control =
    typeof children === "function"
      ? children({ labelId, describedBy })
      : children;

  return (
    <div
      data-testid={testId}
      data-disabled={disabled ? "true" : undefined}
      class={`${styles.row} ${disabled ? styles.disabled : ""}`}
    >
      <div class={styles.text}>
        <div class={styles.labelLine}>
          <span
            id={labelId}
            data-testid={testId ? `${testId}-label` : undefined}
            class={styles.label}
          >
            {label}
          </span>
          <Receipt
            state={receipt}
            testId={testId ? `${testId}-receipt` : undefined}
          />
        </div>
        {description ? (
          <div id={descId} class={styles.description}>
            {description}
          </div>
        ) : null}
      </div>

      <div class={styles.controlCell}>
        <div
          data-testid={testId ? `${testId}-control` : undefined}
          class={styles.control}
        >
          {control}
        </div>
        {error ? (
          <div
            id={errorId}
            role="alert"
            data-testid={testId ? `${testId}-error` : undefined}
            class={styles.error}
          >
            {error}
          </div>
        ) : null}
      </div>
    </div>
  );
}
