/**
 * ProgressBar — en stripe som svarer på spørsmålet operatøren faktisk har:
 * «rekker jeg å hente kaffe?»
 *
 * Prosenten alene svarer ikke på det, og en ubestemt stripe svarer bare «noe
 * skjer». Derfor kommer BÅDE tallet og gjenstående tid fra
 * `@lib/ui/progress-core` — `formatPercent` og `formatEta`, ikke en
 * avrunding skrevet her.
 *
 * `formatEta` sier «beregner …» heller enn å gjette mens estimatet varmer
 * opp, og kvantiserer resten i grove bøtter («ca. 2 min igjen»), fordi
 * «1 min 47 s igjen» er feil på en måte brukeren kan SE. Alt det er kjernens
 * avgjørelser; her brukes de.
 *
 * Kalleren eier estimatoren (`createEtaEstimator`), fordi den hører til
 * jobben og ikke til stripen — en stripe som forsvinner og kommer tilbake
 * skal ikke miste det den har lært om farten.
 */

import { formatEta, formatPercent } from "@lib/ui/progress-core";

import { t, tf } from "../../i18n";
import styles from "./ProgressBar.module.css";

export interface ProgressBarProps {
  /** 0..1. */
  fraction: number;
  /** Millisekunder igjen, eller `null` for «beregner …». */
  etaMs?: number | null;
  /** Hva som pågår, i klartekst. */
  label?: string;
  /** Skjul avlesningen (prosent + gjenstående tid). */
  hideReadout?: boolean;
  testId?: string;
}

export function ProgressBar({
  fraction,
  etaMs = null,
  label,
  hideReadout = false,
  testId,
}: ProgressBarProps) {
  const percent = formatPercent(fraction);
  // `t`/`tf` er skallets reaktive utgaver, så en ETA-linje følger språkbytte
  // uten at jobben må starte på nytt.
  const eta = formatEta(etaMs, t, tf);

  return (
    <div data-testid={testId} class={styles.wrap}>
      {label ? <div class={styles.label}>{label}</div> : null}
      <div
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        aria-valuetext={eta}
        aria-label={label}
        class={styles.track}
      >
        <div class={styles.fill} style={{ width: `${percent}%` }} />
      </div>
      {hideReadout ? null : (
        <div class={styles.readout}>
          <span data-testid={testId ? `${testId}-percent` : undefined}>
            {percent}%
          </span>
          <span data-testid={testId ? `${testId}-eta` : undefined}>{eta}</span>
        </div>
      )}
    </div>
  );
}
