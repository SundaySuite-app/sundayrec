/**
 * Tabs — for VALG innenfor én skjerm, aldri for navigasjon.
 *
 * De tre destinasjonene bor i skinnen; faner som skjuler halve appen er
 * nøyaktig arkitekturen «Frivilligen først» river ned. Det som blir igjen er
 * små, ekte fanevalg — Klipp · Lyd · Eksporter i editoren — der alle
 * alternativene hører til samme oppgave og er synlige samtidig.
 *
 * ## Piltastene er ikke pynt
 *
 * WAI-ARIAs fanemønster har ÉN tabstopp for hele stripen: Tab hopper inn og
 * ut, piltastene flytter mellom fanene. Uten det må en tastaturbruker tabbe
 * gjennom hver eneste fane for å komme til innholdet — som er verre enn ingen
 * faner. `tabIndex` er derfor −1 på alle unntatt den valgte, og
 * ArrowLeft/Right/Home/End flytter valget og fokus sammen.
 */

import { useRef } from "preact/hooks";
import type { JSX } from "preact";

import styles from "./Tabs.module.css";

export interface TabItem {
  id: string;
  label: string;
  /** Tilstandsmerke: fanen er unnagjort (editorens steg). */
  done?: boolean;
  /**
   * Tegnet i sirkelen foran etiketten — «1», «2», «✓».
   *
   * Finnes for editorens STEG (canvasens `.steps .k`), der rekkefølgen er en
   * del av det man leser: «1 Klipp» sier at det er noe etter. En vanlig
   * fanestripe utelater den, og da tegnes ingen sirkel.
   *
   * `aria-hidden` på sirkelen: nummeret er posisjon, ikke navn, og
   * `role="tab"` forteller allerede «1 av 3».
   */
  step?: string;
}

export interface TabsProps {
  items: readonly TabItem[];
  value: string;
  onChange: (id: string) => void;
  /** Navn på stripen for skjermlesere. */
  label: string;
  testId?: string;
}

export function Tabs({ items, value, onChange, label, testId }: TabsProps) {
  const ref = useRef<HTMLDivElement | null>(null);

  const move = (delta: number, absolute?: number): void => {
    const index = items.findIndex((item) => item.id === value);
    const next =
      absolute !== undefined
        ? absolute
        : // Rundt: fra siste til første og motsatt. En stripe som stopper i
          // enden krever at man vet hvor man er, som er den ene tingen en
          // tastaturbruker ikke ser.
          (index + delta + items.length) % items.length;
    const target = items[Math.max(0, Math.min(items.length - 1, next))];
    if (!target) return;
    onChange(target.id);
    ref.current
      ?.querySelector<HTMLButtonElement>(`[data-tab="${target.id}"]`)
      ?.focus();
  };

  const onKeyDown = (
    event: JSX.TargetedKeyboardEvent<HTMLDivElement>,
  ): void => {
    const map: Record<string, () => void> = {
      ArrowRight: () => move(1),
      ArrowLeft: () => move(-1),
      Home: () => move(0, 0),
      End: () => move(0, items.length - 1),
    };
    const handler = map[event.key];
    if (!handler) return;
    event.preventDefault();
    handler();
  };

  return (
    <div
      ref={ref}
      role="tablist"
      aria-label={label}
      data-testid={testId}
      class={styles.tabs}
      onKeyDown={onKeyDown}
    >
      {items.map((item) => {
        const selected = item.id === value;
        return (
          <button
            key={item.id}
            type="button"
            role="tab"
            data-tab={item.id}
            data-testid={testId ? `${testId}-row-${item.id}` : undefined}
            aria-selected={selected ? "true" : "false"}
            tabIndex={selected ? 0 : -1}
            class={`${styles.tab} ${selected ? styles.on : ""} ${
              item.done ? styles.done : ""
            }`}
            onClick={() => onChange(item.id)}
          >
            {item.step ? (
              <span aria-hidden="true" class={styles.k}>
                {item.step}
              </span>
            ) : null}
            {item.label}
          </button>
        );
      })}
    </div>
  );
}
