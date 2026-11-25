/**
 * PLAN — den ukentlige tidsplanen. Jobb nr. 4 av fire, og den som får appen til
 * å starte opptak av seg selv.
 *
 * ## Hva som bor her (F6)
 *
 *   • `AutoRecordCard` — bryteren «Ta opp automatisk» og «Start automatisk med
 *     maskinen». De to er ett spørsmål: skjer det planlagte opptaket, også
 *     etter en omstart?
 *   • `WeekView`       — uka som rutenett: mandag–søndag som kolonner, tid
 *     nedover, hvert faste opptak som en blokk du drar og strekker. Erstattet
 *     (F6) skjemaet med ÉN fast tid og listen «Flere tider».
 *   • `WakeCard`       — vekkingen av maskinen, som holder opptaket i live.
 *
 * ## Presentasjon og regler er skilt
 *
 * Alt som SKRIVER til tidsplanen går gjennom de rene kjernene `plan-core.ts`
 * (geometri, snapping, overlapp, slot-skriving), `schedule-core.ts` (planen
 * som data: varigheter, ukedager) og `slots-core.ts` (listen med lagret indeks,
 * vekkeordene). Denne fila og kortene er tynne lag over dem.
 */

import { useEffect } from "preact/hooks";

import { t } from "../../i18n";
import { route } from "../../router/router";
import { SettingsSection } from "../settings/SettingsSection";
import { FirstRunResumeChip } from "../setup/FirstRunResumeChip";
import styles from "../setup/setup.module.css";
import { AutoRecordCard } from "./AutoRecordCard";
import { WakeCard } from "./WakeCard";
import { WeekView } from "./WeekView";

export function PlanPage() {
  const current = route.value;
  const anchor = current.anchor;
  const highlight = current.highlight === true;

  // En lenke inn hit kan peke på et sted INNE i siden (`?goto=plan:schedule`).
  // Effekten henger på hele ruten, ikke ankerstrengen: to trykk på samme lenke
  // skal rulle to ganger.
  useEffect(() => {
    if (!anchor) return;
    const id = requestAnimationFrame(() => {
      document.getElementById(anchor)?.scrollIntoView({ block: "start" });
    });
    return () => cancelAnimationFrame(id);
  }, [current, anchor]);

  return (
    <div class={styles.page} data-testid="plan-page">
      <FirstRunResumeChip />
      <p data-testid="plan-lede" class={styles.lede}>
        {t("app.plan.lede")}
      </p>
      <AutoRecordCard highlight={highlight && anchor === "auto"} />
      <WeekView />
      <SettingsSection
        id="sec-schedule"
        sectionKey="schedule"
        intro={t("app.setup.advanced.schedDesc")}
      >
        <WakeCard />
      </SettingsSection>
    </div>
  );
}
