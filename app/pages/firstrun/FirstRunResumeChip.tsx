/**
 * «Fortsett oppsettet» — fører tilbake til første gang, på steget man sto på.
 *
 * Rendret på OPPTAK, PLAN og INNSTILLINGER: bunnlinja står under første gang
 * også, så en frivillig kan forlate sekvensen midt i. Posisjonen ligger i
 * `firstRunIndex` (et modulsignal), så chippen trenger bare å åpne ruten.
 *
 * Aldri den vanlige `Chip` (`ui/Chip/Chip.tsx`): den er dokumentert som
 * ikke-klikkbar, og denne MÅ være klikkbar. Derfor et ekte `<button>`.
 *
 * `null` når `!showFirstRunResumeChip(...)`. Ingen lukk-knapp: den forsvinner
 * av seg selv når første gang er fullført (`onboardingDone`).
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
