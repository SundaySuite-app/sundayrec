/**
 * Button — og den ene regelen som gjør den verdt en egen fil:
 *
 * ## En sperret knapp må si HVORFOR
 *
 * Legacy-skallet har grå knapper overalt, og en frivillig som trykker på en av
 * dem får ingenting — ingen bevegelse, ingen forklaring, ingen anelse om hva
 * som mangler. «Start opptak» er den verste: den er grå fordi ingen lydkilde
 * er valgt, og det står ingen steder.
 *
 * Så `disabled` alene finnes ikke her. En knapp som er av tar `disabledReason`,
 * og den grunnen ender tre steder samtidig:
 *
 *   - `title`            → verktøytips for musa,
 *   - `aria-describedby` → skjermleseren leser den etter etiketten,
 *   - en skjult `<span>` → som er det `aria-describedby` peker på.
 *
 * ## `aria-disabled`, ikke `disabled`
 *
 * Et ekte `disabled`-attributt tar knappen ut av tabrekkefølgen, og da kan en
 * tastaturbruker ikke engang komme fram til den for å HØRE hvorfor den er av.
 * `aria-disabled` + en klikkhåndterer som ikke gjør noe gir samme resultat for
 * musa og en langt bedre forklaring for alle andre.
 *
 * ## `record` er en egen variant
 *
 * Rødt er reservert for at det tas opp (canvasens sett 0). Derfor er
 * opptaksknappen sin egen variant og ikke «primary med rød farge» — ingen
 * annen knapp i appen kan komme til å arve den.
 */

import type { ComponentChildren, JSX } from "preact";
import { useId } from "preact/hooks";

import styles from "./Button.module.css";

export type ButtonVariant =
  "primary" | "secondary" | "ghost" | "danger" | "record";

export interface ButtonProps {
  children: ComponentChildren;
  variant?: ButtonVariant;
  size?: "md" | "lg";
  onClick?: (event: JSX.TargetedMouseEvent<HTMLButtonElement>) => void;
  /** En skrivning eller en jobb er i gang — knappen tar ikke imot mer. */
  busy?: boolean;
  /** Av. Krever `disabledReason`; se toppen av fila. */
  disabled?: boolean;
  /**
   * Hvorfor knappen er av, i klartekst. Påkrevd i praksis: uten den er dette
   * en grå knapp uten forklaring, som er tilstanden vi bygger oss bort fra.
   */
  disabledReason?: string;
  /** Ikon-slot foran etiketten. */
  icon?: ComponentChildren;
  /** Fyll bredden den får. */
  block?: boolean;
  type?: "button" | "submit";
  testId?: string;
  /**
   * Knappen folder ut noe: `aria-expanded` + `aria-controls`.
   *
   * D2s kontrollrom folder skjermer ut på stedet, og kilde-kortets «Endre» er
   * en slik knapp uten å være en `ControlCard`-rad. Uten de to attributtene
   * ville en skjermleserbruker fått en knapp som «gjør noe» og en ny landmasse
   * som dukket opp uten forklaring.
   */
  expanded?: boolean;
  /** Id-en `expanded` styrer. Utelates når knappen ikke folder noe ut. */
  controls?: string;
  /** Videreført på roten — DialogHost hviler på `data-dialog-button`. */
  "data-dialog-button"?: string;
}

const VARIANT: Record<ButtonVariant, string> = {
  primary: styles.primary,
  secondary: styles.secondary,
  ghost: styles.ghost,
  danger: styles.danger,
  record: styles.record,
};

export function Button({
  children,
  variant = "secondary",
  size = "md",
  onClick,
  busy = false,
  disabled = false,
  disabledReason,
  icon,
  block = false,
  type = "button",
  testId,
  expanded,
  controls,
  "data-dialog-button": dialogButton,
}: ButtonProps) {
  const reasonId = useId();
  // «Opptatt» er en form for «av»: begge betyr «ikke trykk nå», og begge må
  // stoppe klikket. Bare den ene har en grunn å fortelle.
  const off = disabled || busy;
  const reason = disabled ? disabledReason : undefined;

  return (
    <button
      type={type}
      data-testid={testId}
      data-variant={variant}
      data-dialog-button={dialogButton}
      aria-disabled={off ? "true" : undefined}
      aria-busy={busy ? "true" : undefined}
      aria-expanded={expanded}
      aria-controls={expanded === undefined ? undefined : controls}
      aria-describedby={reason ? reasonId : undefined}
      title={reason}
      class={[
        styles.btn,
        VARIANT[variant],
        size === "lg" ? styles.lg : "",
        block ? styles.block : "",
        off ? styles.off : "",
      ]
        .filter(Boolean)
        .join(" ")}
      onClick={(event) => {
        // Ikke `disabled`-attributtet: se toppen av fila. Klikket stoppes her
        // i stedet, så knappen fortsatt kan fokuseres og forklares.
        if (off) {
          event.preventDefault();
          return;
        }
        onClick?.(event);
      }}
    >
      {icon ? (
        <span aria-hidden="true" class={styles.icon}>
          {icon}
        </span>
      ) : null}
      <span>{children}</span>
      {reason ? (
        <span id={reasonId} class={styles.reason}>
          {reason}
        </span>
      ) : null}
    </button>
  );
}
