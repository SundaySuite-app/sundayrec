/**
 * EmptyState — SAMME form overalt, og alltid tre ting:
 *
 *   1. hva som mangler,
 *   2. hvorfor det betyr noe,
 *   3. den ENE knappen som kan gjøre noe med det.
 *
 * Én handling, ikke to. Canvasens sett 7 viser fire tomtilstander ved siden av
 * hverandre («Ingen lagringsmappe», «Ingen kamera», «Ingen lydenheter», «Fil
 * kan ikke åpnes») nettopp for å vise at de har samme form: en frivillig som
 * har lest én av dem vet hvordan de andre virker. To likestilte knapper ville
 * gjort hver enkelt til et lite valg å ta stilling til.
 *
 * En tomtilstand UTEN handling er lov (papirkurven som er tom er ikke et
 * problem som skal løses) — men da er det ingen knapp i det hele tatt, ikke en
 * grå.
 */

import type { ComponentChildren } from "preact";

import styles from "./EmptyState.module.css";

export interface EmptyStateProps {
  title: string;
  description?: string;
  /** Den ene handlingen. En `<Button>`, ikke to. */
  action?: ComponentChildren;
  testId?: string;
}

export function EmptyState({
  title,
  description,
  action,
  testId,
}: EmptyStateProps) {
  return (
    <div data-testid={testId} class={styles.empty}>
      <b
        data-testid={testId ? `${testId}-title` : undefined}
        class={styles.title}
      >
        {title}
      </b>
      {description ? (
        <span
          data-testid={testId ? `${testId}-description` : undefined}
          class={styles.description}
        >
          {description}
        </span>
      ) : null}
      {action ? <span class={styles.action}>{action}</span> : null}
    </div>
  );
}
