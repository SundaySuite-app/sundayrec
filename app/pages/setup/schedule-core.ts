/**
 * «Ta opp automatisk» — ÉN ukentlig tid, oversatt til og fra `settings.slots`.
 *
 * ## Hva modellen faktisk er
 *
 * Bakenden kjenner `ScheduleSlot { days[], start, stop, max }` — OG, siden
 * P1b, `autoRecordEnabled`. Før det flagget fantes var «finnes det en slot?»
 * den eneste måten å stave «av» på, så bryteren måtte SLETTE tidspunktet for å
 * slå seg av: en bryter som kaster data den ikke viser. P1a skrev det ned som
 * en eiersak; eieren svarte, og nøkkelen finnes nå
 * (`Settings::auto_record_enabled`, lest ett sted — `Settings::active_slots`).
 *
 * Så: `autoRecordOn` er flagget OG en tid. «Av» rører ikke tidene, og «på
 * igjen» henter dem tilbake fra basen og ikke fra en økt-hukommelse.
 *
 * ## Varighet, ikke stopptidspunkt
 *
 * Canvasen spør «90 min», ikke «11:00–12:30». Det er samme tall sett fra den
 * som skal svare: en gudstjeneste varer halvannen time, den slutter ikke
 * halv ett. Regnestykket mellom de to formene bor her, med midnatt håndtert —
 * en kveldsgudstjeneste 23:30 + 90 min er 01:00, ikke et negativt tall.
 */

import type { ScheduleSlot } from "@lib/../bindings/ScheduleSlot";

import type { Settings } from "../../state/settings";

/**
 * Er automatisk opptak PÅ akkurat nå?
 *
 * To ting, ikke én: flagget må stå, og det må finnes en tid å planlegge. Et
 * armert flagg uten en eneste slot er ikke «på» — det er en bryter som lover
 * et opptak ingen har bedt om, og bakenden ville hatt ingenting å gjøre.
 */
export function autoRecordOn(settings: Settings): boolean {
  return (
    settings.autoRecordEnabled !== false && (settings.slots ?? []).length > 0
  );
}

/** Ukedag: 0 = mandag … 6 = søndag. Samme tall som `ScheduleSlot.days`. */
export type Weekday = 0 | 1 | 2 | 3 | 4 | 5 | 6;

export const WEEKDAYS: readonly Weekday[] = [0, 1, 2, 3, 4, 5, 6];

/** Én ukentlig tid, slik siden spør om den. */
export interface WeeklyPlan {
  day: Weekday;
  /** `HH:MM`, lokal veggklokke. */
  start: string;
  /** Varighet i minutter. */
  minutes: number;
}

/**
 * Søndag 11:00, 90 minutter.
 *
 * Samme dag og klokkeslett som legacy-slot-redigereren foreslår
 * (`openSlotEditor`: `{ days: [6], start: '11:00', stop: '12:00' }`), men 90
 * minutter og ikke 60: en høymesse er halvannen time, og et opptak som stopper
 * midt i den siste salmen er verre enn ett som har ti minutter på slutten.
 */
export const DEFAULT_PLAN: WeeklyPlan = { day: 6, start: "11:00", minutes: 90 };

/** Varighetene siden tilbyr. `min` er det brukeren leser. */
export const DURATION_CHOICES: readonly number[] = [
  30, 45, 60, 75, 90, 105, 120, 150, 180,
];

const HHMM = /^([01]?\d|2[0-3]):[0-5]\d$/;

/** Er dette et klokkeslett bakenden kan arme en vekking på? */
export function isValidTime(value: string): boolean {
  return HHMM.test(value);
}

/** `HH:MM` → minutter siden midnatt. `null` for noe som ikke er et tidspunkt. */
export function minutesOfDay(value: string): number | null {
  if (!isValidTime(value)) return null;
  const [h, m] = value.split(":").map(Number);
  return h * 60 + m;
}

/** Minutter siden midnatt → `HH:MM`, med døgnet rundt. */
export function timeOfDay(minutes: number): string {
  const wrapped = ((Math.round(minutes) % 1440) + 1440) % 1440;
  const h = Math.floor(wrapped / 60);
  const m = wrapped % 60;
  return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
}

/** Stopptidspunktet en start + varighet gir. Krysser midnatt uten å klage. */
export function stopFor(start: string, minutes: number): string {
  const base = minutesOfDay(start);
  if (base === null) return start;
  return timeOfDay(base + Math.max(1, Math.round(minutes)));
}

/** Varigheten mellom to klokkeslett. Stopp ≤ start betyr at det krysser
 *  midnatt, akkurat som legacy `updateSlotDurationDisplay` regner det. */
export function durationBetween(start: string, stop: string): number {
  const a = minutesOfDay(start);
  const b = minutesOfDay(stop);
  if (a === null || b === null) return DEFAULT_PLAN.minutes;
  return b > a ? b - a : 1440 - a + b;
}

/**
 * Planen `settings.slots` beskriver, eller `null` når automatisk opptak er av.
 *
 * Leser BARE den første sloten: nivå 1 eier én ukentlig tid, og en profil med
 * flere er noe Avansert skal vise (fase P1b). Dagen er den første valgte —
 * en slot kan i prinsippet ha flere dager, og da er det ærligere å vise den
 * første enn å finne på et sammendrag.
 */
export function planFromSlots(
  slots: readonly ScheduleSlot[] | null | undefined,
): WeeklyPlan | null {
  const first = slots?.[0];
  if (!first) return null;
  const day = (first.days ?? []).find((d) => d >= 0 && d <= 6);
  return {
    day: (day ?? DEFAULT_PLAN.day) as Weekday,
    start: isValidTime(first.start) ? first.start : DEFAULT_PLAN.start,
    minutes: durationBetween(first.start, first.stop),
  };
}

/**
 * Slot-listen en plan gir, med alt annet urørt.
 *
 * De øvrige slotene beholdes med vilje: de er ikke nivå 1 sine, og en skjerm
 * skal ikke slette data den ikke viser. `max` fra den gamle sloten følger med
 * av samme grunn — det er en grense noen satte, og varigheten her er ikke den
 * samme tingen.
 */
export function slotsFromPlan(
  plan: WeeklyPlan,
  existing: readonly ScheduleSlot[] | null | undefined,
): ScheduleSlot[] {
  const rest = (existing ?? []).slice(1);
  const previous = existing?.[0];
  return [
    {
      days: [plan.day],
      start: plan.start,
      stop: stopFor(plan.start, plan.minutes),
      max: previous?.max ?? null,
    },
    ...rest,
  ];
}
