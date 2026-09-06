/**
 * «Fortsett oppsettet» — R6: gjenåpner sjekklisten «Sett opp» forlot.
 *
 * Rendret på to sider, ikke bare på OPPTAK: fire av de fem radene sender
 * «Sett opp» dit, men kirkeraden sender den til INNSTILLINGER (se
 * `Checklist` i `FirstRun.tsx`), og en frivillig som lander der skal se
 * samme vei tilbake som en som lander på OPPTAK. Én komponent, to
 * innsettingssteder — se `RecordPage.tsx` og `SetupPage.tsx`.
 *
 * Aldri den vanlige `Chip` (`ui/Chip/Chip.tsx`): den er DOKUMENTERT som
 * ikke-klikkbar («en brikke som kan trykkes på er en knapp som ser ut som en
 * etikett»), og denne MÅ være klikkbar. Derfor et ekte `<button>`, i samme
 * gul-tonede pille-språk som `Chip`s `gold`-tone, men med egen hover/fokus.
 *
 * `null` når `!showFirstRunResumeChip(...)` — se `firstrun-core.ts`. Ingen
 * lukk-knapp: chippen skal ikke kunne avvises og glemmes, den skal forsvinne
 * av seg selv når «Åpne SundayRec» setter `onboardingDone`.
 */

import { t } from "../../i18n";
import { settings } from "../../state/settings";
import { resumeFirstRun } from "./FirstRun";
import { showFirstRunResumeChip } from "./firstrun-core";
import styles from "./FirstRunResumeChip.module.css";

export function FirstRunResumeChip() {
  if (!showFirstRunResumeChip(settings.value.onboardingDone)) return null;

  return (
    <button
      type="button"
      data-testid="first-run-resume"
      class={styles.chip}
      onClick={() => resumeFirstRun()}
    >
      {t("app.first.resumeChip")}
    </button>
  );
}
