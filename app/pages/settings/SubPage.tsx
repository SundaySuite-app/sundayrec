/**
 * Rammen rundt en innstillingsskjerm: én setning (leden) som sier hva skjermen
 * er for, og så selve skjermen.
 *
 * Brukes av de samme skjermene på to steder: INNSTILLINGER (en seksjon hver) og
 * første gang (`FirstRun`, én skjerm om gangen). Leden er den eneste teksten
 * som sier hva skjermen er for, så den står begge steder.
 */

import type { ComponentChildren } from "preact";

import styles from "./settings.module.css";

export interface SubPageProps {
  /** Én setning: hva skjermen er for. */
  lede: string;
  children: ComponentChildren;
  testId?: string;
}

export function SubPage({ lede, children, testId }: SubPageProps) {
  return (
    <div data-testid={testId} class={styles.sub}>
      <p
        data-testid={testId ? `${testId}-lede` : undefined}
        class={styles.lede}
      >
        {lede}
      </p>
      {children}
    </div>
  );
}
