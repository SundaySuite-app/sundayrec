/**
 * ControlCard — én rad i kontrollrommet: hva den heter, hva den står på nå, og
 * veien til å endre det UTEN å forlate skjermen.
 *
 *     ┌──────────────────────────────────────────────┬───────────┐
 *     │ HVOR SKAL OPPTAKENE?                         │  [Endre]  │
 *     │ /Users/frivillig/SundayRec                   │           │
 *     └──────────────────────────────────────────────┴───────────┘
 *
 * ## Hvorfor ikke `DecisionCard`
 *
 * `DecisionCard` er en NUMMERERT rad i en liste man går ovenfra og ned én gang
 * — den er sjekklisten første gang, og den lever videre i `FirstRun`. Her er
 * det ingen rekkefølge å komme gjennom: kontrollrommet er skjermen man står på
 * hver søndag, og kortet skal si ett faktum og folde seg ut på stedet. Et
 * nummer her ville lovet en rekkefølge som ikke finnes.
 *
 * ## Utfoldingen er ett sted, ikke to
 *
 * Knappen bærer `aria-expanded` og `aria-controls`, og kroppen har id-en den
 * peker på. Uten det ville en skjermleserbruker fått en knapp som «gjør noe»
 * og en ny landmasse som dukket opp uten forklaring — som er nøyaktig hva
 * canvasens «kort folder seg ut på stedet» ser ut som uten semantikken.
 *
 * `onExpand` UTELATT betyr at kortet ikke kan foldes: de to tilleggene (kamera
 * og «Ta opp automatisk») styres av bryteren sin, og en utfoldingsknapp ved
 * siden av en bryter som allerede åpner kroppen er to affordanser for det ene.
 *
 * ## `id` er ankeret
 *
 * `?goto=settings:audio` → `record#sound`, og `RecordPage` ruller hit og folder
 * ut. `id` + `data-anchor` er den bare kontrakten (samme som `Card`), så siden
 * ikke trenger å kjenne en CSS-selektor. `data-highlight` er pulsen når man
 * KOM hit — ikke pynt: å lande et sted uten å skjønne hvorfor er feilmodusen
 * ankeret finnes for å unngå.
 */

import type { ComponentChildren } from "preact";

import styles from "./ControlCard.module.css";

export type ControlCardTone = "neutral" | "warn";

export interface ControlCardProps {
  /** Ankeret: `id` + `data-anchor` på roten. Også standard-testid-en. */
  id: string;
  /** Hva raden HETER — spørsmålet, i det små. */
  title: string;
  /** Svaret som gjelder nå, stort. Det er dette man kommer for å lese. */
  value: string;
  tone?: ControlCardTone;
  /** Er kroppen åpen? */
  expanded: boolean;
  /** Utelatt ⇒ ingen utfoldingsknapp — se toppen av fila. */
  onExpand?: () => void;
  /** Teksten på utfoldingsknappen når kortet er LUKKET («Endre»/«Sett opp»). */
  expandLabel?: string;
  /** …og når det er åpent («Lukk»). */
  collapseLabel?: string;
  /** Pulsen: man kom nettopp hit fra en lenke. */
  highlight?: boolean;
  /** Til venstre for teksten — bryteren, når kortet har en. */
  lead?: ComponentChildren;
  /** Mellom teksten og knappen — kvitteringen, når kortet har en. */
  trail?: ComponentChildren;
  /** Kroppen. Rendres bare når `expanded`. */
  children?: ComponentChildren;
  /** Standard: `control-<id>`. Kamera og auto beholder sine egne. */
  testId?: string;
}

export function ControlCard({
  id,
  title,
  value,
  tone = "neutral",
  expanded,
  onExpand,
  expandLabel,
  collapseLabel,
  highlight = false,
  lead,
  trail,
  children,
  testId,
}: ControlCardProps) {
  const test = testId ?? `control-${id}`;
  const bodyId = `${id}-body`;
  return (
    <section
      id={id}
      data-anchor={id}
      data-testid={test}
      data-tone={tone}
      data-expanded={expanded ? "true" : "false"}
      data-highlight={highlight ? "true" : undefined}
      class={`${styles.card} ${tone === "warn" ? styles.warn : ""} ${
        highlight ? styles.pulse : ""
      }`}
    >
      <div class={styles.head}>
        {lead ? <div class={styles.lead}>{lead}</div> : null}
        <div class={styles.grow}>
          <div data-testid={`${test}-title`} class={styles.title}>
            {title}
          </div>
          <div data-testid={`${test}-summary`} class={styles.value}>
            {value}
          </div>
        </div>
        {trail}
        {onExpand ? (
          <button
            type="button"
            aria-expanded={expanded}
            aria-controls={bodyId}
            data-testid={`${test}-expand`}
            class={styles.expand}
            onClick={onExpand}
          >
            {expanded ? collapseLabel : expandLabel}
          </button>
        ) : null}
      </div>

      {expanded ? (
        <div id={bodyId} data-testid={`${test}-body`} class={styles.body}>
          {children}
        </div>
      ) : null}
    </section>
  );
}
