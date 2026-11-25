/**
 * PLAN — den ukentlige tidsplanen. Jobb nr. 4 av fire, og den som får appen til
 * å starte opptak av seg selv.
 *
 * ## Hva som bor her (F5)
 *
 * Fram til F5 var «Ta opp automatisk» et kort i kontrollrommet på TA OPP, og
 * «Flere tider» + vekkingen lå under Innstillinger → Tidsplan. De to var
 * halvdeler av det samme spørsmålet — «når skal appen ta opp av seg selv?» —
 * og eieren ville ha dem på ett sted, med et eget navn i navigasjonen.
 *
 *   • `AutoRecordCard` — bryteren og ÉN fast ukentlig tid (dag, klokkeslett,
 *     varighet), med «start med maskinen».
 *   • `ScheduleCard`   — flere faste tider og vekkingen av maskinen.
 *
 * ## Presentasjon og regler er skilt
 *
 * F6 bytter begge kortene mot en grafisk ukevisning. Alt som SKRIVER til
 * tidsplanen går gjennom de rene kjernene `schedule-core.ts` (planen som
 * data: `planFromSlots`/`slotsFromPlan`, varigheter, ukedager) og
 * `slots-core.ts` (listen med lagret indeks, `withSlot`/`withoutIndex`, vekke-
 * ordene). Denne fila og kortene er tynne lag over dem; den nye visningen kan
 * kalle de samme funksjonene uten å arve en eneste JSX-linje herfra.
 */

import { useEffect } from "preact/hooks";

import { t } from "../../i18n";
import { route } from "../../router/router";
import { SettingsSection } from "../settings/SettingsSection";
import { FirstRunResumeChip } from "../setup/FirstRunResumeChip";
import styles from "../setup/setup.module.css";
import { AutoRecordCard } from "./AutoRecordCard";
import { ScheduleCard } from "./ScheduleCard";

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
      <SettingsSection
        id="sec-schedule"
        sectionKey="schedule"
        intro={t("app.setup.advanced.schedDesc")}
      >
        <ScheduleCard />
      </SettingsSection>
    </div>
  );
}
