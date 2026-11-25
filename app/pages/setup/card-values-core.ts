/**
 * Kompaktverdiene til de to tilleggene som er brytere og ikke ett av de fem
 * spørsmålene: «Ta med kamera» (INNSTILLINGER) og «Ta opp automatisk» (PLAN).
 *
 * De har ingen `Decision` i `decisions-core.ts`, så deres kompaktverdi er egne
 * fakta, med samme form som `Answer`: en NØKKEL og innsettingene, aldri en
 * ferdig setning. En kjerne som kalte `t()` ville trukket katalogen inn i
 * node-gaten og gjort hver regel avhengig av hvilken tekst noen skrev sist.
 *
 * (Fram til F5 het fila `record/control-core.ts` og eide også kortlista i
 * kontrollrommet på TA OPP. Kortene bor på INNSTILLINGER og PLAN nå, så
 * ankerlista og stabelradene er borte.)
 */

import type { Settings } from "../../state/settings";
import {
  autoRecordOn,
  planFromSlots,
  type WeeklyPlan,
} from "../plan/schedule-core";

// ── Kamera ──────────────────────────────────────────────────────────────────

/**
 * Kompaktverdien på kamera-kortet.
 *
 * `listError` er ikke det samme som `none`, og det er hele grunnen feltet
 * finnes: de to har hvert sitt neste steg — en kabel å sjekke, eller en
 * tillatelse å gi. Shimmen svarte før med tom liste på begge.
 */
export type CameraValue =
  | { key: "off" }
  | { key: "listError" }
  | { key: "none" }
  | { key: "noneChosen" }
  | { key: "name"; name: string };

export interface CameraFacts {
  /** `videoEnabled` — er tillegget på i det hele tatt? */
  enabled: boolean;
  /** Navnet som står lagret, trimmet. Tom streng = ingen. */
  chosen: string;
  /** Antall kameraer bakenden ser. `null` = ikke lest ennå (ikke «ingen»). */
  count: number | null;
  /** Lesningen FEILET — se over. */
  failed: boolean;
}

export function cameraValue(facts: CameraFacts): CameraValue {
  // AV: kortet sier hva tillegget ER, ikke hvilket kamera som ville blitt
  // brukt. Et kameranavn på et avslått tillegg leses som «dette skjer».
  if (!facts.enabled) return { key: "off" };
  if (facts.failed) return { key: "listError" };
  if (facts.count === 0) return { key: "none" };
  if (facts.chosen) return { key: "name", name: facts.chosen };
  return { key: "noneChosen" };
}

/** Kan kamera-kortet foldes ut? Bare når tillegget er PÅ — kroppen er
 *  kameravalget, og et valg mellom enheter som ikke skal brukes er en kontroll
 *  uten virkning. Bryteren i topplinja er affordansen når det er av. */
export function cameraExpandable(facts: CameraFacts): boolean {
  return facts.enabled;
}

// ── «Ta opp automatisk» ─────────────────────────────────────────────────────

/** Kompaktverdien på auto-kortet: tiden som gjelder, eller at ingen er satt. */
export type AutoValue =
  { key: "off" } | { key: "plan"; day: number; start: string; minutes: number };

export function autoValue(settings: Settings): AutoValue {
  // Flagget OG en tid — `autoRecordOn` er den ene regelen, og den bor i
  // `schedule-core` sammen med resten av tidsplanen.
  if (!autoRecordOn(settings)) return { key: "off" };
  const plan: WeeklyPlan | null = planFromSlots(settings.slots ?? []);
  if (!plan) return { key: "off" };
  return {
    key: "plan",
    day: plan.day,
    start: plan.start,
    minutes: plan.minutes,
  };
}

/** Kan auto-kortet foldes ut? Samme regel som kamera. */
export function autoExpandable(settings: Settings): boolean {
  return autoValue(settings).key === "plan";
}
