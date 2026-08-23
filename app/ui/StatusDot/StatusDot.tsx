/**
 * StatusDot — åtte piksler som er appens eneste tilstandsfarge.
 *
 * Fem toner, og de betyr fem forskjellige ting (canvasens sett 1):
 *
 *   `good`    grønn   alt er som det skal
 *   `warn`    gul     du må gjøre noe FØR søndag
 *   `rec`     rød     det tas opp NÅ — og rødt betyr aldri noe annet
 *   `listen`  gull    vi hører etter (VU-en er i live, ingenting tas opp)
 *   `neutral` grå     ingen påstand
 *
 * `rec` er den eneste som pulserer, fordi den er den eneste som beskriver noe
 * som skjer akkurat nå. Pulsen er dessuten ren pynt: fargen og setningen ved
 * siden av sier det samme, så `prefers-reduced-motion` (base.css) slår den av
 * uten at noe går tapt.
 */

import styles from "./StatusDot.module.css";

export type DotTone = "good" | "warn" | "rec" | "listen" | "neutral";

export interface StatusDotProps {
  tone: DotTone;
  testId?: string;
}

const TONE: Record<DotTone, string> = {
  good: styles.good,
  warn: styles.warn,
  rec: styles.rec,
  listen: styles.listen,
  neutral: styles.neutral,
};

export function StatusDot({ tone, testId }: StatusDotProps) {
  return (
    <span
      // Dekorativ: setningen ved siden av prikken er det en skjermleser skal
      // lese. To stemmer som sier det samme er verre enn én.
      aria-hidden="true"
      data-tone={tone}
      data-testid={testId}
      class={`${styles.dot} ${TONE[tone]}`}
    />
  );
}
