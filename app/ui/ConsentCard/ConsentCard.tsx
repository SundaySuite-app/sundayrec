/**
 * Samtykkekortet — canvasens sett 7.1.
 *
 * ## Hvorfor det er et KORT på Opptak og ikke et steg i første-gangs-løpet
 *
 * Canvasens sett 6 sier det rett ut: «Diagnostikk-samtykket er ikke et steg
 * lenger; det spørres én gang som kort første gang appen står på Opptak.»
 * Legacys veiviser har det som steg 5 av 6, midt mellom «test at lyden
 * fungerer» og «alt er klart» — altså et personvernvalg klemt inn i en
 * oppsettsrekke, der det leses som enda et steg å komme seg forbi.
 *
 * ## To likeverdige knapper
 *
 * «Nei takk» er en vanlig sekundærknapp, ikke en grå tekstlenke, og den står
 * FØRST. Et frivillig valg der den ene siden er en knapp og den andre er noe
 * som ser ut som fint trykk er ikke frivillig. Legacy gjør det samme, og
 * `telemetry-consent-copy-core.ts` har en test som forbyr «vi anbefaler» og
 * «vennligst» i teksten av nøyaktig samme grunn.
 *
 * ## «Vi spør deg om igjen»
 *
 * `needsPrompt` er sann i TO tilfeller: ingen har svart ennå, og omfanget har
 * blitt utvidet siden forrige svar — også for den som sa nei. De to er ikke det
 * samme spørsmålet, og et kort som stilte det første til en som allerede har
 * svart ville underslått hvorfor det kom tilbake.
 *
 * Hvilken av dem det er avgjøres av `promptCopyFor` i
 * `@lib/telemetry-consent-copy-core` — den samme rene kjernen legacys
 * oppstartskort bruker, med sin egen test som forbyr «vi anbefaler» og
 * «vennligst» i teksten. Vi låner AVGJØRELSEN, ikke nøkkelen: en variabel med
 * en hel katalognøkkel i ville vært usynlig for `check-i18n-keys.mjs`, så
 * setningen slås opp med en literal.
 *
 * ## Kortet forsvinner bare når svaret ER lagret
 *
 * `telemetry_consent_set` svarer `null` når IPC-en feilet, og da er spørsmålet
 * ikke besvart. Hele poenget med «spør én gang» er at et TAPT svar må spørres
 * om på nytt; å skjule kortet uansett ville sagt til brukeren at valget er
 * registrert når det ikke er det.
 */

import { useState } from "preact/hooks";

import { FIRST_ASK, promptCopyFor } from "@lib/telemetry-consent-copy-core";
import type { ConsentStatus } from "@legacy/bindings/ConsentStatus";

import { t } from "../../i18n";
import { Button } from "../Button/Button";
import styles from "./ConsentCard.module.css";

export interface ConsentCardProps {
  /**
   * Bakendens `consent.status`. Avgjør om dette er det FØRSTE spørsmålet eller
   * et gjentatt — se toppen av fila. `undefined` behandles som første gang.
   */
  status?: ConsentStatus;
  /** «Hva sendes?» — verten eier forhåndsvisningen. */
  onExplain: () => void;
  /** Svaret er lagret hos bakenden. Verten skjuler kortet. */
  onAnswered: () => void;
  testId?: string;
}

export function ConsentCard({
  status,
  onExplain,
  onAnswered,
  testId = "consent-card",
}: ConsentCardProps) {
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);

  async function answer(granted: boolean): Promise<void> {
    if (busy) return;
    setBusy(true);
    setFailed(false);
    try {
      const result = await window.api
        .telemetryConsentSet(granted)
        .catch(() => null);
      if (result) onAnswered();
      else setFailed(true);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section data-testid={testId} class={styles.card}>
      <h2 data-testid={`${testId}-title`} class={styles.title}>
        {t("app.consent.title")}
      </h2>
      <p data-testid={`${testId}-description`} class={styles.description}>
        {t("app.consent.desc")}
      </p>
      {promptCopyFor(status ?? "never-asked") === FIRST_ASK ? null : (
        <p data-testid={`${testId}-reask`} class={styles.description}>
          {t("onboarding.rePromptDesc")}
        </p>
      )}
      {failed ? (
        <p role="alert" data-testid={`${testId}-error`} class={styles.error}>
          {t("general.saveFailed")}
        </p>
      ) : null}
      <div class={styles.actions}>
        <Button
          variant="secondary"
          busy={busy}
          testId={`${testId}-no`}
          onClick={() => void answer(false)}
        >
          {t("app.consent.no")}
        </Button>
        <Button
          variant="primary"
          busy={busy}
          testId={`${testId}-yes`}
          onClick={() => void answer(true)}
        >
          {t("app.consent.yes")}
        </Button>
        <Button variant="ghost" testId={`${testId}-what`} onClick={onExplain}>
          {t("app.consent.what")}
        </Button>
      </div>
    </section>
  );
}
