/**
 * Chip — en liten faktabrikke ved siden av noe annet.
 *
 * «Video», «Eksportert», «Anbefalt», «Avbrutt». Aldri en knapp: en brikke som
 * kan trykkes på er en knapp som ser ut som en etikett, og en frivillig som
 * har lært at brikkene ikke gjør noe slutter å prøve.
 *
 * Prikken er valgfri og hører til der brikken beskriver en TILSTAND
 * («Tar opp») heller enn et faktum («Video»).
 */

import type { ComponentChildren } from "preact";

import { StatusDot, type DotTone } from "../StatusDot/StatusDot";
import styles from "./Chip.module.css";

export type ChipTone = "neutral" | "good" | "warn" | "bad" | "gold" | "rec";

export interface ChipProps {
  children: ComponentChildren;
  tone?: ChipTone;
  /** Vis en StatusDot foran teksten. */
  dot?: DotTone;
  testId?: string;
}

const TONE: Record<ChipTone, string> = {
  neutral: styles.neutral,
  good: styles.good,
  warn: styles.warn,
  bad: styles.bad,
  gold: styles.gold,
  rec: styles.rec,
};

export function Chip({ children, tone = "neutral", dot, testId }: ChipProps) {
  return (
    <span
      data-tone={tone}
      data-testid={testId}
      class={`${styles.chip} ${TONE[tone]}`}
    >
      {dot ? <StatusDot tone={dot} /> : null}
      <span>{children}</span>
    </span>
  );
}
