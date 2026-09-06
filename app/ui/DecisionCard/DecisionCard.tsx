/**
 * DecisionCard — ett spørsmål, svaret som står nå, og veien til å endre det.
 *
 *     ┌───┬──────────────────────────────────────────┬───────────┐
 *     │ 1 │ Hvilken lyd?                             │  [Endre]  │
 *     │   │ Behringer X32 · kanal 15–16              │           │
 *     │   │ ✓ Vi hører lyd                           │           │
 *     ├───┴──────────────────────────────────────────┴───────────┤
 *     │ … hele «Hvilken lyd?»-skjermen, når raden er foldet ut … │
 *     └──────────────────────────────────────────────────────────┘
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
 *
 * ## F2-T4: knappen kan folde ut i stedet for å navigere
 *
 * `onExpand` gjør raden til en `ControlCard`s slektning: den samme knappen, med
 * den samme etiketten, åpner skjermen PÅ STEDET i stedet for å forlate
 * sekvensen. `onAction` er alternativet — én av de to, aldri begge, ellers har
 * raden to affordanser for det ene.
 *
 * Semantikken er den samme som kontrollrommets, og den er ikke pynt: knappen
 * bærer `aria-expanded` + `aria-controls` (`Button` har dem som props), og
 * kroppen har id-en den peker på. Uten det er «kortet folder seg ut på stedet»
 * en knapp som «gjør noe» og en ny landmasse som dukket opp uten forklaring.
 *
 * Kroppen er i tillegg en NAVNGITT gruppe med `tabindex="-1"`, og fokus
 * flyttes dit ved utfolding — samme grep som kvitteringen på OPPTAK
 * (`RecordPage`s `Done`). En tastaturbruker som trykker «Sett opp» skal høre
 * hva som åpnet seg, ikke stå igjen på en knapp mens skjermen vokste under
 * henne. `aria-labelledby` peker på spørsmålet raden allerede viser: en
 * etikett som ble skrevet en gang til er en etikett som kan si noe annet.
 *
 * ⚠️ Kroppen RENDRES bare når `expanded`. Det er ikke en optimalisering — den
 * innbygde `SoundPage` holder en VU-måler, og monteringen er det som avgjør om
 * appen ber om lydenheten. Se VU-regelen i `FirstRun.tsx`.
 */

import type { ComponentChildren } from "preact";
import { useEffect, useRef } from "preact/hooks";

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
  /**
   * Knappen NAVIGERER. Utelates når raden folder ut i stedet (`onExpand`).
   */
  onAction?: () => void;
  /**
   * Knappen FOLDER UT, på stedet. Utelates når raden navigerer.
   *
   * Med den satt bytter etiketten til `collapseLabel` når kroppen står åpen —
   * en knapp som fortsatt sa «Sett opp» over en åpen skjerm ville vært en
   * knapp som beskriver seg selv feil.
   */
  onExpand?: () => void;
  /** Er kroppen åpen? Bare meningsfull sammen med `onExpand`. */
  expanded?: boolean;
  /** Teksten på knappen når kroppen står åpen («Lukk»). */
  collapseLabel?: string;
  /** Kroppen. Rendres BARE når `expanded` — se toppen av fila. */
  children?: ComponentChildren;
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
  onExpand,
  expanded = false,
  collapseLabel,
  children,
  anchor,
  testId,
}: DecisionCardProps) {
  const todo = status === "todo";
  const bodyId = testId ? `${testId}-body` : undefined;
  const questionId = testId ? `${testId}-question` : undefined;
  const panel = useRef<HTMLDivElement | null>(null);

  // Fokus flyttes inn ved UTFOLDINGEN, ikke på hver gjengivelse mens kortet
  // står åpent: det innbygde skjemaet skriver innstillinger, og en effekt som
  // hentet fokus tilbake for hvert tastetrykk ville tatt markøren ut av feltet
  // brukeren skriver i.
  const wasExpanded = useRef(expanded);
  useEffect(() => {
    const opened = expanded && !wasExpanded.current;
    wasExpanded.current = expanded;
    if (opened) panel.current?.focus();
  }, [expanded]);

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
      data-expanded={onExpand ? (expanded ? "true" : "false") : undefined}
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
        <div id={questionId} data-testid={questionId} class={styles.question}>
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
        onClick={onExpand ?? onAction}
        expanded={onExpand ? expanded : undefined}
        // Bare når kroppen FINNES: den rives ut av treet ved kollaps, og en
        // `aria-controls` som peker på ingenting er en referanse en
        // skjermleser ikke kan følge.
        controls={expanded ? bodyId : undefined}
        testId={testId ? `${testId}-action` : undefined}
      >
        {onExpand && expanded ? (collapseLabel ?? actionLabel) : actionLabel}
      </Button>

      {onExpand && expanded ? (
        <div
          ref={panel}
          id={bodyId}
          role="group"
          aria-labelledby={questionId}
          tabIndex={-1}
          data-testid={bodyId}
          class={styles.panel}
        >
          {children}
        </div>
      ) : null}
    </section>
  );
}
