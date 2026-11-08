/**
 * ActionBar — handlingslinja nederst på en side.
 *
 * Start (Opptak), «Neste» (redigering, første gang) og eksportknappen skal
 * stå der man så dem sist, uansett hvor lang siden er eller hvor lite vinduet
 * er. Før denne lå de i innholdet og havnet under bunnlinja i et vindu på
 * 760 px. Tetthet var det forrige svaret (D3); linja er det riktige.
 *
 * Den er `position: sticky` i `#main` (det er `<main>` som ruller, ikke
 * dokumentet), og må derfor stå som SISTE barn av sidens rot-element — sticky
 * holder seg innenfor foreldren sin.
 *
 * `hint` er en dempet linje til venstre (snarveien, hvorfor knappen er av);
 * barna er knappene, til høyre.
 */

import type { ComponentChildren } from "preact";

import styles from "./ActionBar.module.css";

export interface ActionBarProps {
  children: ComponentChildren;
  /** Dempet tekst til venstre for knappene. */
  hint?: ComponentChildren;
  testId?: string;
}

export function ActionBar({ children, hint, testId }: ActionBarProps) {
  return (
    <div data-testid={testId ?? "action-bar"} class={styles.bar}>
      <div class={styles.hint}>{hint}</div>
      <div class={styles.actions}>{children}</div>
    </div>
  );
}
