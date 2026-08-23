/**
 * Card — den ene beholderen. Alt som skal skille seg fra bakgrunnen gjør det
 * på denne måten, i denne radiusen, med denne kanten.
 *
 * `tone` er ikke pynt. En gul kant betyr «du må gjøre noe før søndag», en rød
 * betyr «noe gikk galt», en grønn «dette er i orden» og `selected` «dette er
 * valget som gjelder». Fordi tonene betyr noe, må de være få — fem, og ikke
 * en sjette fordi en skjerm trengte «litt annerledes».
 *
 * `anchor` er den bare id-en ruteren navigerer til (`?goto=…` → `route.anchor`).
 * Kortet setter `id` og `data-anchor`, og en side kan rulle dit uten å kjenne
 * en CSS-selektor. Pulsen når man KOMMER dit hører til sidene (fase P) — her
 * er bare stedet å komme til.
 */

import type { ComponentChildren } from "preact";

import styles from "./Card.module.css";

export type CardTone = "neutral" | "warn" | "bad" | "good" | "selected";

export interface CardProps {
  children?: ComponentChildren;
  /** Overskrift. Utelates når kortet bare er en flate rundt noe annet. */
  title?: string;
  /** Én setning under tittelen. */
  description?: string;
  /** Knapper til høyre i topplinjen. */
  actions?: ComponentChildren;
  tone?: CardTone;
  /** Navigasjonsmål: `id` + `data-anchor` på roten. */
  anchor?: string;
  testId?: string;
}

const TONE: Record<CardTone, string> = {
  neutral: "",
  warn: styles.warn,
  bad: styles.bad,
  good: styles.good,
  selected: styles.selected,
};

export function Card({
  children,
  title,
  description,
  actions,
  tone = "neutral",
  anchor,
  testId,
}: CardProps) {
  return (
    <section
      id={anchor}
      data-anchor={anchor}
      data-tone={tone}
      data-testid={testId}
      class={`${styles.card} ${TONE[tone]}`}
    >
      {title || description || actions ? (
        <header class={styles.head}>
          <div class={styles.grow}>
            {title ? (
              <h2
                data-testid={testId ? `${testId}-title` : undefined}
                class={styles.title}
              >
                {title}
              </h2>
            ) : null}
            {description ? (
              <p
                data-testid={testId ? `${testId}-description` : undefined}
                class={styles.description}
              >
                {description}
              </p>
            ) : null}
          </div>
          {actions ? <div class={styles.actions}>{actions}</div> : null}
        </header>
      ) : null}
      {children}
    </section>
  );
}
