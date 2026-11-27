/**
 * GearRow — ÉN rad i tannhjulet: ikon, ett ord (og gjerne en statuslinje under
 * det), kontrollene til høyre, og en valgfri «ekstra»-linje under alt det
 * andre (installer-knappen, releasenotatet).
 *
 * Raden er en kropp og ingen knapp: kontrollene i den er knappene. Et trykk
 * på hele raden ville ikke betydd noe, og en rad som ser trykkbar ut uten å
 * være det er en frivillig som har lært at tannhjulet ikke svarer.
 */

import type { ComponentChildren } from "preact";

import { route } from "../../../router/router";
import { Icon, type IconName } from "../../../ui/Icon/Icon";
import styles from "./gear.module.css";

export interface GearRowProps {
  icon: IconName;
  title: string;
  /** Statuslinjen under ordet. */
  sub?: ComponentChildren;
  controls?: ComponentChildren;
  extra?: ComponentChildren;
  testId: string;
  /** Ankrene (`?goto=settings:<anker>`) som peker på denne raden. */
  anchors: readonly string[];
}

export function GearRow({
  icon,
  title,
  sub,
  controls,
  extra,
  testId,
  anchors,
}: GearRowProps) {
  const r = route.value;
  // Kom man hit for DENNE raden, får den den vanlige gullpulsen — samme regel
  // som resten av appen (`Route.highlight`).
  const hit =
    r.anchor !== undefined &&
    r.highlight !== false &&
    anchors.includes(r.anchor);
  return (
    <div
      data-testid={testId}
      data-anchor-hit={hit ? "true" : undefined}
      class={`${styles.row} ${hit ? styles.spot : ""}`}
    >
      <span aria-hidden="true" class={styles.rowIcon}>
        <Icon name={icon} size={22} strokeWidth={1.8} />
      </span>
      <span class={styles.rowText}>
        <span class={styles.rowTitle}>{title}</span>
        {sub}
      </span>
      {controls ? <span class={styles.rowControls}>{controls}</span> : null}
      {extra ? <div class={styles.rowExtra}>{extra}</div> : null}
    </div>
  );
}
