/**
 * Receipt — «Lagret ✓», og de tre andre tingene den kan si.
 *
 * Hele appen har ÉN lagringsmodell (`useSetting`): alt anvendes med det
 * samme, og kvitteringen er beviset. Den er derfor det eneste stedet en
 * frivillig får vite at endringen faktisk landet — det finnes ingen
 * Lagre-knapp å slippe.
 *
 * `role="status"` og ikke `aria-live="assertive"`: kvitteringen skal leses
 * opp NÅR skjermleseren er ferdig med det den holder på med, ikke avbryte.
 *
 * `idle` rendrer ingenting og opptar likevel plassen sin
 * (`min-height`/`min-width` i CSS-en) — ellers ville hele raden hoppe hver
 * gang noe ble lagret, og det er akkurat i det øyeblikket brukeren ser dit.
 */

import { t } from "../../i18n";
import type { Receipt as ReceiptState } from "../../settings/use-setting-core";
import styles from "./Receipt.module.css";

export interface ReceiptProps {
  state: ReceiptState;
  testId?: string;
}

export function Receipt({ state, testId }: ReceiptProps) {
  const text =
    state === "saving"
      ? t("app.receipt.saving")
      : state === "saved"
        ? t("app.receipt.saved")
        : state === "failed"
          ? t("app.receipt.failed")
          : "";

  return (
    <span
      role="status"
      data-state={state}
      data-testid={testId}
      class={`${styles.receipt} ${state === "failed" ? styles.failed : ""}`}
    >
      {text}
    </span>
  );
}
