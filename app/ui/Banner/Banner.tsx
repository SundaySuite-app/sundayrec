/**
 * Banner — den brede stripen øverst på en side, for det som ikke kan vente.
 *
 * To toner, og bare to (canvasens sett 7):
 *
 *   `bad`  — noe gikk TAPT, og setningen sier når. «Opptaket ble avbrutt
 *            kl. 11:42» — tidspunktet er halve informasjonen.
 *   `warn` — noe trenger deg FØR søndag. Ingenting er ødelagt ennå.
 *
 * Ingen `info`-tone. En blå stripe som ikke krever noe blir tapetet folk slutter
 * å lese, og da forsvinner de to som betyr noe sammen med den. Ting som bare
 * er verdt å nevne er en toast.
 *
 * `role="alert"` for `bad` (avbryter — noe er tapt), `role="status"` for
 * `warn` (venter på tur — det haster ikke i sekunder).
 */

import type { ComponentChildren } from "preact";

import { t } from "../../i18n";
import { Button } from "../Button/Button";
import styles from "./Banner.module.css";

export interface BannerProps {
  tone: "bad" | "warn";
  title: string;
  /** Detaljen: hva som skjedde, når, og hva som er lagret. */
  detail?: string;
  /** Knapper til høyre. */
  actions?: ComponentChildren;
  /** Lukkekryss. Utelates når stripen ikke skal kunne avvises. */
  onDismiss?: () => void;
  testId?: string;
}

export function Banner({
  tone,
  title,
  detail,
  actions,
  onDismiss,
  testId,
}: BannerProps) {
  return (
    <div
      role={tone === "bad" ? "alert" : "status"}
      data-tone={tone}
      data-testid={testId}
      class={`${styles.banner} ${tone === "bad" ? styles.bad : styles.warn}`}
    >
      <div class={styles.text}>
        <div class={styles.title}>{title}</div>
        {detail ? <div class={styles.detail}>{detail}</div> : null}
      </div>
      <div class={styles.actions}>
        {actions}
        {onDismiss ? (
          <Button
            variant="ghost"
            testId={testId ? `${testId}-dismiss` : undefined}
            onClick={onDismiss}
          >
            {t("app.common.dismiss")}
          </Button>
        ) : null}
      </div>
    </div>
  );
}
