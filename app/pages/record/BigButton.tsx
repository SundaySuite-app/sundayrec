/**
 * Den store opptaksknappen («Én flate»): 232 px pustende glorie, 184 px
 * innerste sirkel med rød prikk. Den lille varianten (96 px, nederst på
 * kamerabildet) har samme kontrakt.
 *
 * `aria-disabled` og ALDRI `disabled`: det ekte attributtet tar knappen ut av
 * tabrekkefølgen, og da kunne en tastaturbruker ikke komme fram til den for å
 * HØRE hvorfor den er av (`reason` står som `title` og `aria-describedby`).
 */

import { useId } from "preact/hooks";

import { t } from "../../i18n";
import styles from "./BigButton.module.css";

export interface BigButtonProps {
  onStart: () => void;
  /** Har Start lov til å gjøre noe? */
  canStart: boolean;
  /** Start er i gang — knappen tar ikke imot mer. */
  busy?: boolean;
  /** Hvorfor knappen er av, i klartekst. */
  reason?: string;
  /** `small` = 96 px over kamerabildet. */
  size?: "big" | "small";
}

export function BigButton({
  onStart,
  canStart,
  busy = false,
  reason,
  size = "big",
}: BigButtonProps) {
  const reasonId = useId();
  const off = !canStart || busy;
  const blocked = !canStart;
  const label = t("app.record.start");
  return (
    <span class={`${styles.wrap} ${size === "small" ? styles.small : ""}`}>
      {size === "big" && canStart ? (
        <span aria-hidden="true" class={styles.halo} />
      ) : null}
      <button
        type="button"
        data-testid="record-start"
        data-variant="record"
        aria-label={label}
        aria-disabled={off ? "true" : undefined}
        aria-busy={busy ? "true" : undefined}
        aria-describedby={blocked && reason ? reasonId : undefined}
        title={blocked && reason ? reason : label}
        class={`${styles.button} ${off ? styles.off : ""}`}
        onClick={(event) => {
          if (off) {
            event.preventDefault();
            return;
          }
          onStart();
        }}
      >
        <span aria-hidden="true" class={styles.dot} />
        {blocked && reason ? (
          <span id={reasonId} class={styles.reason}>
            {reason}
          </span>
        ) : null}
      </button>
    </span>
  );
}
