/**
 * Gate — «denne delen kan ikke gjøre noe nå, og her er hvorfor».
 *
 * SundayRec kan sendes ut uten bakenden en flate trenger: e-postsending ligger
 * bak en cargo-feature, og en build uten den har ingen sendevei uansett hva
 * brukeren skriver. Fram til `feature-gate-core` fantes så en slik flate ut
 * NØYAKTIG som en som virket — med en «Send test» som rapporterte en feil den
 * fant på selv. En frivillig kan ikke skille «du har satt det opp feil» fra
 * «dette finnes ikke i denne versjonen», og bruker lørdagskvelden på å prøve.
 *
 * Avgjørelsen er ikke her: `mapGate` fra `@lib/ui/feature-gate-core` sier om
 * det skal vises et banner, om innholdet skal slås av, og hvilken tekst
 * brikken får. Denne komponenten maler svaret.
 *
 * ## `inert` og ikke `disabled`
 *
 * Barna er vilkårlige — kort, rader, felt. Å gå gjennom dem og sette
 * `disabled` på hver kontroll ville krevd at Gate kjente hvert barn. `inert`
 * på beholderen slår av HELE undertreet på én gang: ikke klikkbart, ikke
 * fokuserbart, ikke lesbart for skjermlesere. Grunnen står utenfor det inerte
 * området, så den kan fortsatt leses.
 *
 * ⚠️ `mapGate` gir aldri `ok` sammen med `disabled` — en gate som er synlig
 * når alt virker blir tapetet folk lærer å ignorere.
 */

import type { ComponentChildren } from "preact";

import { mapGate, type GateStatus } from "@lib/ui/feature-gate-core";

import { Chip } from "../Chip/Chip";
import styles from "./Gate.module.css";

export interface GateProps {
  status: GateStatus;
  /** Kort brikketekst, oversatt. Kjernen har norske standarder som reserve. */
  chipText?: string;
  /** Én–to setninger: hva som mangler, og hvem som kan fikse det. */
  explanation?: string;
  /** Hvor man ser videre. */
  docsHint?: string;
  children: ComponentChildren;
  testId?: string;
}

export function Gate({
  status,
  chipText,
  explanation,
  docsHint,
  children,
  testId,
}: GateProps) {
  const view = mapGate({ status, chipText, explanation, docsHint });

  return (
    <div data-testid={testId} data-gate={view.variant} class={styles.gate}>
      {view.showBanner ? (
        <div
          data-testid={testId ? `${testId}-banner` : undefined}
          class={styles.note}
        >
          <Chip tone="warn">{view.chipText}</Chip>
          <span class={styles.explanation}>
            {view.explanation}
            {view.docsHint ? (
              <span class={styles.hint}>{view.docsHint}</span>
            ) : null}
          </span>
        </div>
      ) : null}
      <div
        // JSX-prop og ikke `setAttribute` i en effekt: en re-render ville
        // strøket et attributt satt utenfra, stille, og bare noen ganger.
        inert={view.disabled}
        data-testid={testId ? `${testId}-content` : undefined}
        class={view.disabled ? styles.dimmed : undefined}
      >
        {children}
      </div>
    </div>
  );
}
