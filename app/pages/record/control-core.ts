/**
 * Kontrollrommet, som DATA: hvilke kort som står der, og hva hvert av dem
 * svarer akkurat nå.
 *
 * ## Hvorfor en ren fil til
 *
 * `decisions-core.ts` avgjør allerede om et spørsmål er besvart og hva svaret
 * ER. Den fila er ikke duplisert her — den er GJENBRUKT. Det denne legger til
 * er de tre tingene kontrollrommet trenger og som ingen beslutning har:
 *
 *   1. **Hvilke kort, i hvilken rekkefølge.** Ankeret ruteren lover
 *      (`?goto=settings:audio` → `record#sound`) må være et kort som faktisk
 *      finnes. `isControlId` er den ene lista begge sider holdes mot, og
 *      `app/router/router.test.ts` har en rad som krysser dem.
 *   2. **De to tilleggene har ingen `Decision`.** Kamera og «Ta opp
 *      automatisk» er ikke ett av de fem spørsmålene — de er brytere. Deres
 *      kompaktverdi er derfor egne fakta, med samme form som `Answer`: en
 *      NØKKEL og innsettingene, aldri en ferdig setning. En kjerne som kalte
 *      `t()` ville trukket katalogen inn i node-gaten og gjort hver regel
 *      avhengig av hvilken tekst noen skrev sist.
 *   3. **Tonen.** Et kort er gult når spørsmålet ikke er besvart — samme regel
 *      som `DecisionCard`, og den bor her i stedet for i en JSX-linje.
 *
 * ⚠️ `unknown` er IKKE gult. Enhetslisten og ledig diskplass leses asynkront
 * etter at siden er malt, og et gult kort som blir nøytralt etter 100 ms er
 * nøyaktig det som lærer folk å ignorere gult. Se toppen av `decisions-core`.
 */

import type { Answer, Decision, DecisionId } from "../setup/decisions-core";
import {
  autoRecordOn,
  planFromSlots,
  type WeeklyPlan,
} from "../setup/schedule-core";
import type { Settings } from "../../state/settings";

/**
 * De seks kortene kontrollrommet har.
 *
 * `sound` står i venstrekolonnen (den LEVENDE halvdelen — kilde, måler, Start);
 * de fem andre i høyre. Rekkefølgen i `STACK_IDS` er rekkefølgen på skjermen.
 */
export type ControlId =
  "sound" | "folder" | "quality" | "camera" | "auto" | "notify";

/** Alle seks, som et sett å holde et anker mot. */
export const CONTROL_IDS: readonly ControlId[] = [
  "sound",
  "folder",
  "quality",
  "camera",
  "auto",
  "notify",
];

/** Kortstabelen i HØYRE kolonne, i rekkefølge. `sound` er ikke med — den er
 *  venstrekolonnens levende kort. */
export const STACK_IDS: readonly ControlId[] = [
  "folder",
  "quality",
  "camera",
  "auto",
  "notify",
];

/** De tre av kortene som er ett av de fem spørsmålene og derfor har en
 *  `Decision`. (`sound` har en også, men venstrekolonnen tegner den selv.) */
export const STACK_DECISIONS: readonly DecisionId[] = [
  "folder",
  "quality",
  "notify",
];

/** Er dette et kort — altså et anker `RecordPage` kan folde ut? */
export function isControlId(
  value: string | null | undefined,
): value is ControlId {
  return (
    value !== null &&
    value !== undefined &&
    (CONTROL_IDS as readonly string[]).includes(value)
  );
}

/** Gul eller nøytral. `unknown` er nøytralt — se toppen av fila. */
export type ControlTone = "neutral" | "warn";

export function toneOf(decision: Decision): ControlTone {
  return decision.status === "todo" ? "warn" : "neutral";
}

/**
 * Én rad i stabelen, som data: svaret som står nå, tonen, og hva knappen
 * heter. `answer` er `decisions-core` sin egen — kallstedet slår den opp med
 * `decision-text.ts`, det ene stedet den oversettelsen bor.
 */
export interface ControlRow {
  id: DecisionId;
  answer: Answer;
  tone: ControlTone;
  /** «Sett opp» (ikke noe svar står) eller «Endre». */
  needsSetUp: boolean;
}

/** Radene for de kortene som er beslutninger, i `STACK_DECISIONS`-rekkefølge. */
export function decisionRows(decisions: readonly Decision[]): ControlRow[] {
  const rows: ControlRow[] = [];
  for (const id of STACK_DECISIONS) {
    const decision = decisions.find((d) => d.id === id);
    if (!decision) continue;
    rows.push({
      id,
      answer: decision.answer,
      tone: toneOf(decision),
      // Samme regel som nivå 1 hadde: «Sett opp» bare når det bokstavelig talt
      // ikke står et svar. En mappe som er valgt, men der disken ikke har
      // svart ennå, er noe man ENDRER.
      needsSetUp:
        decision.answer.key === "notSetUp" || decision.answer.key === "nobody",
    });
  }
  return rows;
}

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
