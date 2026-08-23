/**
 * Rammen rundt de fem undersidene: én setning som sier hva skjermen er for,
 * og veien tilbake.
 *
 * «Tilbake» er en ekte knapp og ikke bare skinnen: skinnen tar deg til
 * OPPSETT-destinasjonen, men en frivillig som står inne i «Hvilken lyd?» har
 * ikke noe språk for at det er samme sted. Knappen sier det.
 */

import type { ComponentChildren } from "preact";

import { t } from "../../i18n";
import { navigate } from "../../router/router";
import { Button } from "../../ui/Button/Button";
import styles from "./setup.module.css";

export interface SubPageProps {
  /** Én setning: hva skjermen er for. */
  lede: string;
  children: ComponentChildren;
  testId?: string;
}

export function SubPage({ lede, children, testId }: SubPageProps) {
  return (
    <div data-testid={testId} class={styles.sub}>
      <div class={styles.subHead}>
        <p class={styles.lede}>{lede}</p>
        <Button
          variant="ghost"
          testId="setup-back"
          onClick={() => navigate("setup")}
        >
          {t("app.setup.back")}
        </Button>
      </div>
      {children}
    </div>
  );
}
