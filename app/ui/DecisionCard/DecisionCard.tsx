/**
 * DecisionCard — ett spørsmål, svaret som står nå, og veien til å endre det.
 *
 *     ┌───┬──────────────────────────────────────────┬───────────┐
 *     │ 1 │ Hvilken lyd?                             │  [Endre]  │
 *     │   │ Behringer X32 · kanal 15–16              │           │
 *     │   │ ✓ Vi hører lyd                           │           │
 *     └───┴──────────────────────────────────────────┴───────────┘
 *
 * Canvasens `.dec` (sett 5). Tre ting gjør den til noe annet enn et `Card` med
 * tekst i:
 *
 * **Nummeret er en tilstand, ikke pynt.** Rekkefølgen er svarrekkefølgen — en
 * frivillig som aldri har sett appen skal kunne gå ovenfra og ned. Sirkelen er
 * gullfylt når spørsmålet er besvart og gul-kantet når det ikke er det, så
 * «hvor langt har vi kommet» kan leses på en meters avstand.
 *
 * **Spørsmålet er ETIKETTEN, svaret er verdien.** Motsatt av dagens app, der
 * korttittelen er innstillingens navn («Lagringsmappe») og verdien står med
 * liten grå skrift under. Her er spørsmålet lite og svaret stort, fordi det er
 * svaret man kommer for å lese.
 *
 * **Knappen sier hva den gjør.** «Endre» når det finnes noe å endre, «Sett
 * opp» når det ikke gjør det. Én knapp, aldri null — et kort uten vei videre
 * er et kort som bare kritiserer.
 */

import type { ComponentChildren } from "preact";

import { Button } from "../Button/Button";
import styles from "./DecisionCard.module.css";

export type DecisionCardStatus = "done" | "todo" | "unknown";

export interface DecisionCardProps {
  /** 1-basert. Det brukeren teller. */
  number: number;
  /** Spørsmålet, som etikett. */
  question: string;
  /** Svaret som gjelder nå, stort. */
  answer: string;
  /** Én linje under svaret: hvorfor det holder, eller hva som mangler. */
  detail?: ComponentChildren;
  status: DecisionCardStatus;
  /** Knappeteksten — «Endre» eller «Sett opp». */
  actionLabel: string;
  onAction: () => void;
  /** Navigasjonsmål: `id` + `data-anchor` på roten. */
  anchor?: string;
  testId?: string;
}

export function DecisionCard({
  number,
  question,
  answer,
  detail,
  status,
  actionLabel,
  onAction,
  anchor,
  testId,
}: DecisionCardProps) {
  const todo = status === "todo";
  return (
    <section
      id={anchor}
      data-anchor={anchor}
      data-testid={testId}
      data-status={status}
      // `data-tone` i tillegg til `data-status`: statuslinjen, kortene og
      // brikkene bruker allerede «tone» som ordet for farge, og et e2e-spec
      // skal kunne spørre om det ene ordet overalt.
      data-tone={todo ? "warn" : "neutral"}
      class={`${styles.dec} ${todo ? styles.todo : ""}`}
    >
      <span
        aria-hidden="true"
        data-testid={testId ? `${testId}-number` : undefined}
        class={styles.num}
      >
        {number}
      </span>

      <div class={styles.body}>
        <div
          data-testid={testId ? `${testId}-question` : undefined}
          class={styles.question}
        >
          {question}
        </div>
        <div
          data-testid={testId ? `${testId}-answer` : undefined}
          class={styles.answer}
        >
          {answer}
        </div>
        {detail ? (
          <div
            data-testid={testId ? `${testId}-detail` : undefined}
            class={styles.detail}
          >
            {detail}
          </div>
        ) : null}
      </div>

      <Button
        variant={todo ? "primary" : "secondary"}
        onClick={onAction}
        testId={testId ? `${testId}-action` : undefined}
      >
        {actionLabel}
      </Button>
    </section>
  );
}
