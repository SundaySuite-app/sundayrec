/**
 * ToastHost — stabelen nede til høyre.
 *
 * Modellen (køen, tidene, «Angre») ligger i `app/ui/toast.ts`; her er bare
 * flaten. Delingen er det som gjør at `useSetting` kan testes uten et
 * dokument: kjernen kaller `toast()` på ekte, og en node-test leser
 * `toasts.value`.
 *
 * ## Tidene kommer fra huset, ikke herfra
 *
 * `DEFAULT_MS` i modellen er kopiert fra `legacy/renderer/ui/toast.ts`,
 * inkludert den ene som betyr noe: en FEIL forsvinner ikke av seg selv
 * (`error: 0`). Den ene meldingen du ikke har råd til å gå glipp av skal ikke
 * være den som forsvinner mens du ser en annen vei.
 *
 * ## `aria-live="polite"`, ikke `assertive`
 *
 * En toast er en kvittering, ikke en alarm. `assertive` avbryter det
 * skjermleseren holder på med — og under en gudstjeneste er det som regel noe
 * viktigere. Det som MÅ avbryte er et `Banner` med `role="alert"`.
 *
 * ## Ikke inert
 *
 * Verten er søsken av `#app`, ikke inne i det, så en toast overlever at
 * DialogHost slår av appen bak seg. Det er med vilje: en «Kunne ikke lagre»
 * som kom akkurat idet en dialog åpnet seg skal fortsatt kunne leses.
 */

import { dismissToast, toasts } from "../toast";
import { t } from "../../i18n";
import { Button } from "../Button/Button";
import styles from "./ToastHost.module.css";

export function ToastHost() {
  const queue = toasts.value;
  if (queue.length === 0) return null;

  return (
    <div
      aria-live="polite"
      aria-atomic="false"
      data-testid="toast-host"
      class={styles.stack}
    >
      {queue.map((item) => (
        <div
          key={item.id}
          data-kind={item.kind}
          data-testid={`toast-${item.id}`}
          class={`${styles.toast} ${
            item.kind === "error"
              ? styles.error
              : item.kind === "warn"
                ? styles.warn
                : item.kind === "success"
                  ? styles.success
                  : ""
          }`}
        >
          <span data-testid={`toast-${item.id}-message`} class={styles.message}>
            {item.msg}
          </span>
          {item.action ? (
            <Button
              variant="ghost"
              testId={`toast-${item.id}-action`}
              onClick={() => {
                // Fjern først: handlingen kan navigere, og en toast som blir
                // stående over den nye siden er en melding uten kontekst.
                dismissToast(item.id);
                item.action?.onClick();
              }}
            >
              {item.action.label}
            </Button>
          ) : null}
          <Button
            variant="ghost"
            testId={`toast-${item.id}-dismiss`}
            onClick={() => dismissToast(item.id)}
          >
            {t("app.common.dismiss")}
          </Button>
        </div>
      ))}
    </div>
  );
}
