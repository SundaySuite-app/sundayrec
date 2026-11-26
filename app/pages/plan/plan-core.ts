/**
 * PLAN som ukevisning — geometrien og reglene bak blokkene.
 *
 * Ukevisningen (`WeekView.tsx`) er et tynt lag over dette: hvert eneste
 * spørsmål den stiller — «hvor på skjermen ligger 11:00?», «hva blir tiden når
 * blokken slippes her?», «kolliderer den med noe?» — er en ren funksjon her,
 * uten DOM, så grensene (midnatt, søndag→mandag, 15 minutters minimum) er
 * dekket av vitest og ikke av å dra rundt i appen.
 *
 * ## Modellen
 *
 * Bakenden kjenner `ScheduleSlot { days[], start, stop, max }`. Visningen
 * snakker om OPPTREDENER (`Occurrence`): én slot på én dag, med start og
 * varighet i minutter. En slot med flere dager gir flere opptredener; en
 * opptreden som krysser midnatt tegnes som to SEGMENTER — hode på startdagen,
 * hale på neste dag (søndag → mandag, siden uka går rundt).
 *
 * Alt som SKRIVER går tilbake gjennom `ScheduleSlot` — og gjenbruker
 * `withSlot`/`withoutIndex` (`slots-core`) og `stopFor`/`durationBetween`
 * (`schedule-core`), så det finnes ÉN regel for hvordan en tid blir en slot.
 *
 * ## Overlapp
 *
 * Kjernen (`sundayrec-core::schedule`) har ingen overlappsregel: den regner ut
 * «neste start» per slot, og planleggeren tar opp ÉN ting om gangen. To tider
 * oppå hverandre ville gitt et opptak som starter ved den første og et «savnet»
 * for den andre. Derfor FORBYR visningen overlapp (og sier hvorfor), i stedet
 * for å tegne dem side om side og late som begge skjer. Vegg-i-vegg
 * (stopp = neste start) er lov. Uka er en sirkel: søndag 23:00–01:00 kolliderer
 * med mandag 00:30.
 *
 * ## Minutter
 *
 * «Start» er minutter siden midnatt på opptredenens dag (0–1439). Slutt kan
 * være over 1440: da krysser den midnatt. Varighet er 15–1425 min; 1440 er ikke
 * uttrykkbart (start = stopp er «degenerert» for kjernen, ikke et døgn).
 */

import type { ScheduleSlot } from "@legacy/bindings/ScheduleSlot";

import {
  durationBetween,
  minutesOfDay,
  stopFor,
  timeOfDay,
  type Weekday,
} from "./schedule-core";
import { withoutIndex, withSlot } from "./slots-core";

/** Rutenettet snapper til kvarter. */
const SNAP = 15;
/** Kortest mulige opptak. */
export const MIN_DURATION = 15;
/** Lengst mulige: nesten et døgn (start = stopp er ugyldig). */
export const MAX_DURATION = 1440 - SNAP;
/** Et nytt opptak fra et klikk. Samme som `DEFAULT_PLAN`: en høymesse. */
const DEFAULT_DURATION = 90;
export const DAY_MINUTES = 1440;
const WEEK_MINUTES = 7 * DAY_MINUTES;
/** Høyden på én time i rutenettet. 15 min = 14 px, så en kvarters blokk er
 *  fortsatt et treffbart mål. */
export const HOUR_PX = 56;
/** Ruten åpner på dette klokkeslettet (06:00) — natta er et rull unna. */
const DEFAULT_TOP_MINUTES = 6 * 60;
/** Hvor mange piksler en peker må bevege seg før et trykk er en dra-gest. */
export const DRAG_THRESHOLD_PX = 4;

export type SegmentPart = "whole" | "head" | "tail";

/** Én slot på én dag. */
export interface Occurrence {
  /** Indeksen i `settings.slots` — ikke i noen sortert visning. */
  index: number;
  day: Weekday;
  /** Minutter siden midnatt, 0–1439. */
  start: number;
  /** Minutter, 15–1425. */
  duration: number;
}

/** Hva en slot kan bli, uten identitet. */
export type Placement = Pick<Occurrence, "day" | "start" | "duration">;

/** Navnet på en opptreden: slot-indeks + dag. */
export interface OccurrenceRef {
  index: number;
  day: Weekday;
}

/** Den tegnede delen av en opptreden, i én kolonne. */
export interface Segment {
  occ: Occurrence;
  /** Kolonnen (dagen) segmentet tegnes i. */
  day: Weekday;
  /** Minutter fra kolonnens topp. */
  from: number;
  to: number;
  part: SegmentPart;
  /** Stabil nøkkel: `<slot>-<opptredenens dag>-<del>`. */
  key: string;
}

// ── Små verktøy ─────────────────────────────────────────────────────────────

export function clamp(value: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, value));
}

/** Nærmeste kvarter. */
export function snapMinutes(minutes: number, step: number = SNAP): number {
  return Math.round(minutes / step) * step;
}

export function floorSnap(minutes: number, step: number = SNAP): number {
  return Math.floor(minutes / step) * step;
}

export function ceilSnap(minutes: number, step: number = SNAP): number {
  return Math.ceil(minutes / step) * step;
}

function wrapDay(day: number): Weekday {
  return (((Math.trunc(day) % 7) + 7) % 7) as Weekday;
}

/** Mandag = 0, som i `ScheduleSlot.days`. JS har søndag = 0. */
export function weekdayOf(date: Date): Weekday {
  return wrapDay(date.getDay() + 6);
}

export function minutesOfDate(date: Date): number {
  return date.getHours() * 60 + date.getMinutes();
}

// ── Piksler ↔ tid ───────────────────────────────────────────────────────────

/** Minutter fra kolonnens topp → piksler. */
export function minutesToPx(minutes: number, hourPx: number = HOUR_PX): number {
  return (minutes * hourPx) / 60;
}

/** Piksler fra kolonnens topp → minutter (ikke snappet). */
export function pxToMinutes(px: number, hourPx: number = HOUR_PX): number {
  return (px * 60) / hourPx;
}

/** Høyden på hele døgnet. */
export function dayHeightPx(hourPx: number = HOUR_PX): number {
  return minutesToPx(DAY_MINUTES, hourPx);
}

/** Hvilken kolonne ligger `x` i? `x` er målt fra venstre kant av kolonnene. */
export function dayFromX(x: number, width: number): Weekday {
  if (width <= 0) return 0;
  return clamp(Math.floor((x / width) * 7), 0, 6) as Weekday;
}

// ── Slot ↔ opptreden ────────────────────────────────────────────────────────

/** Gyldige, ulike dager i en slot. */
function distinctDays(slot: ScheduleSlot): Weekday[] {
  const seen = new Set<number>();
  const out: Weekday[] = [];
  for (const d of slot.days ?? []) {
    if (Number.isInteger(d) && d >= 0 && d <= 6 && !seen.has(d)) {
      seen.add(d);
      out.push(d as Weekday);
    }
  }
  return out;
}

/**
 * Klokkeslettet kjernen (`sundayrec-core::schedule::parse_hm`) ender med:
 * `H:M` med timer < 24 og minutter < 60, ellers reservetiden.
 */
export function coreTime(
  value: string | null | undefined,
  fallback: string,
): string {
  const parts = (value ?? "").split(":");
  if ((value ?? "").trim() !== "") {
    const h = /^\s*\d+\s*$/.test(parts[0] ?? "") ? Number(parts[0]) : NaN;
    const m = /^\s*\d+\s*$/.test(parts[1] ?? "") ? Number(parts[1]) : NaN;
    if (h < 24 && m < 60) {
      return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
    }
  }
  return fallback;
}

/** Ligger `now` innenfor plasseringen (på dens dag, over midnatt inkludert)? */
export function placementActiveAt(p: Placement, now: Date): boolean {
  const abs = weekdayOf(now) * DAY_MINUTES + minutesOfDate(now);
  const from = p.day * DAY_MINUTES + p.start;
  const week = 7 * DAY_MINUTES;
  const into = (((abs - from) % week) + week) % week;
  return into < p.duration;
}

/**
 * Alle opptredener i en slot-liste. En slot uten gyldig klokkeslett, eller med
 * start = stopp (kjernen avviser den), tegnes ikke — men røres heller ikke.
 * Uleselige tider tegnes på kjernens reservetid (11:00–12:00).
 */
export function occurrencesOf(
  slots: readonly ScheduleSlot[] | null | undefined,
): Occurrence[] {
  const out: Occurrence[] = [];
  (slots ?? []).forEach((slot, index) => {
    // Samme regel som kjernen (`parse_hm`): en uleselig tid får reservetiden
    // 11:00 / 12:00, så sloten vises der Rust faktisk planlegger den.
    const startText = coreTime(slot.start, "11:00");
    const stopText = coreTime(slot.stop, "12:00");
    if (startText === stopText) return;
    const start = minutesOfDay(startText);
    if (start === null) return;
    const duration = durationBetween(startText, stopText);
    for (const day of distinctDays(slot)) {
      out.push({ index, day, start, duration });
    }
  });
  return out;
}

export function sameRef(a: OccurrenceRef, b: OccurrenceRef): boolean {
  return a.index === b.index && a.day === b.day;
}

/** Slutt i minutter fra startdagens midnatt (> 1440 når den krysser). */
export function endOf(p: Pick<Placement, "start" | "duration">): number {
  return p.start + p.duration;
}

/** Opptredenen som én eller to tegnede deler. */
export function segmentsOf(occ: Occurrence): Segment[] {
  const base = `${occ.index}-${occ.day}`;
  const end = endOf(occ);
  if (end <= DAY_MINUTES) {
    return [
      {
        occ,
        day: occ.day,
        from: occ.start,
        to: end,
        part: "whole",
        key: `${base}-whole`,
      },
    ];
  }
  return [
    {
      occ,
      day: occ.day,
      from: occ.start,
      to: DAY_MINUTES,
      part: "head",
      key: `${base}-head`,
    },
    {
      occ,
      day: wrapDay(occ.day + 1),
      from: 0,
      to: end - DAY_MINUTES,
      part: "tail",
      key: `${base}-tail`,
    },
  ];
}

function segmentsOfAll(occs: readonly Occurrence[]): Segment[] {
  return occs.flatMap(segmentsOf);
}

/** `11:00–12:30`, med midnatt: `23:00–01:00`. */
export function rangeLabel(p: Pick<Placement, "start" | "duration">): string {
  return `${timeOfDay(p.start)}–${timeOfDay(endOf(p))}`;
}

/** Slot-feltene en plassering skrives som. */
export function slotTimes(p: Placement): { start: string; stop: string } {
  const start = timeOfDay(p.start);
  return { start, stop: stopFor(start, p.duration) };
}

// ── Overlapp ────────────────────────────────────────────────────────────────

/** Uka er en sirkel: kolliderer [a, a+da) og [b, b+db) når begge går rundt? */
export function intervalsOverlap(
  a: number,
  da: number,
  b: number,
  db: number,
): boolean {
  for (const shift of [-WEEK_MINUTES, 0, WEEK_MINUTES]) {
    if (a < b + shift + db && b + shift < a + da) return true;
  }
  return false;
}

function absStart(p: Pick<Placement, "day" | "start">): number {
  return p.day * DAY_MINUTES + p.start;
}

export function placementsOverlap(a: Placement, b: Placement): boolean {
  return intervalsOverlap(absStart(a), a.duration, absStart(b), b.duration);
}

/**
 * Første opptreden `cand` kolliderer med, eller `null`. `ignore` er opptredenen
 * som selv flyttes (den kolliderer ikke med seg selv). To opptredener av SAMME
 * slot (en slot med flere dager) sjekkes som alle andre.
 */
export function findConflict(
  occs: readonly Occurrence[],
  cand: Placement,
  ignore?: OccurrenceRef | null,
): Occurrence | null {
  for (const o of occs) {
    if (ignore && sameRef(o, ignore)) continue;
    if (placementsOverlap(o, cand)) return o;
  }
  return null;
}

/**
 * Det lengste opptaket (opp til ønsket varighet, i kvarter) som får plass fra
 * `cand.start`. `null` når ikke engang et minimumsopptak får det.
 */
export function fitDuration(
  occs: readonly Occurrence[],
  cand: Placement,
  ignore?: OccurrenceRef | null,
): Placement | null {
  for (let d = cand.duration; d >= MIN_DURATION; d -= SNAP) {
    const next = { ...cand, duration: d };
    if (!findConflict(occs, next, ignore)) return next;
  }
  return null;
}

// ── Gestene ─────────────────────────────────────────────────────────────────

/**
 * Nytt opptak fra en gest i en tom kolonne.
 *
 *   • et TRYKK (`clicked`) gir standardvarigheten, startet på kvarteret
 *     pekeren står i,
 *   • en DRAGING gir nøyaktig det som ble dratt, rundet ut til hele kvarter
 *     (men aldri under minimum).
 */
export function createPlacement(
  day: Weekday,
  fromMinutes: number,
  toMinutes: number,
  clicked: boolean,
): Placement {
  const lo = Math.min(fromMinutes, toMinutes);
  const hi = Math.max(fromMinutes, toMinutes);
  const start = clamp(floorSnap(lo), 0, DAY_MINUTES - SNAP);
  if (clicked) return { day, start, duration: DEFAULT_DURATION };
  const end = clamp(ceilSnap(hi), start + MIN_DURATION, DAY_MINUTES);
  return { day, start, duration: Math.max(MIN_DURATION, end - start) };
}

/**
 * Flytt: `dayDelta` kolonner og `minuteDelta` minutter (snappes). Starten
 * holdes innenfor dagen — en blokk skifter dag ved å dras til en annen
 * kolonne, ikke ved å renne over midnatt. Varigheten er uendret, så en blokk
 * som krysser midnatt fortsetter å gjøre det.
 */
export function movePlacement(
  p: Placement,
  dayDelta: number,
  minuteDelta: number,
): Placement {
  return {
    day: clamp(p.day + Math.trunc(dayDelta), 0, 6) as Weekday,
    start: clamp(snapMinutes(p.start + minuteDelta), 0, DAY_MINUTES - SNAP),
    duration: p.duration,
  };
}

/**
 * Dra i toppkanten: starten flytter seg, slutten står. Starten kan ikke
 * passere slutten minus minimum, eller gjøre opptaket lengre enn maks.
 */
export function resizeStart(p: Placement, newStart: number): Placement {
  const end = endOf(p);
  const start = clamp(
    snapMinutes(newStart),
    Math.max(0, end - MAX_DURATION),
    Math.min(DAY_MINUTES - SNAP, end - MIN_DURATION),
  );
  return { day: p.day, start, duration: end - start };
}

/**
 * Dra i bunnkanten: slutten flytter seg, starten står. `limit` er det seneste
 * slutten får være (standard: så langt maks varighet tillater, også forbi
 * midnatt; musa sender 1440 for en blokk som ikke krysser midnatt).
 */
export function resizeEnd(
  p: Placement,
  newEnd: number,
  limit: number = p.start + MAX_DURATION,
): Placement {
  const end = clamp(
    snapMinutes(newEnd),
    p.start + MIN_DURATION,
    Math.max(p.start + MIN_DURATION, Math.min(limit, p.start + MAX_DURATION)),
  );
  return { day: p.day, start: p.start, duration: end - p.start };
}

export type PlacementKey = "up" | "down" | "left" | "right" | "grow" | "shrink";

/**
 * Tastaturet. Piler flytter (opp/ned ett kvarter, venstre/høyre én dag);
 * Skift + opp/ned endrer lengden (slutten) ett kvarter. Skift + venstre/høyre
 * er ikke noe — varighet har bare én akse.
 */
export function keyPlacement(p: Placement, key: PlacementKey): Placement {
  switch (key) {
    case "up":
      return movePlacement(p, 0, -SNAP);
    case "down":
      return movePlacement(p, 0, SNAP);
    case "left":
      return movePlacement(p, -1, 0);
    case "right":
      return movePlacement(p, 1, 0);
    case "grow":
      return resizeEnd(p, endOf(p) + SNAP);
    case "shrink":
      return resizeEnd(p, endOf(p) - SNAP);
  }
}

export function samePlacement(a: Placement, b: Placement): boolean {
  return a.day === b.day && a.start === b.start && a.duration === b.duration;
}

// ── Skriving til slot-listen ────────────────────────────────────────────────

/** Slotten og dens dager, hvis `ref` peker på en ekte opptreden. */
function slotAt(
  slots: readonly ScheduleSlot[],
  ref: OccurrenceRef,
): ScheduleSlot | null {
  const slot = slots[ref.index];
  if (!slot) return null;
  return distinctDays(slot).includes(ref.day) ? slot : null;
}

/**
 * Slotten uten `day` (andre dager og ugyldige verdier urørt), eller `null` når
 * det var den siste dagen og slotten skal bort.
 */
function withoutDay(slot: ScheduleSlot, day: Weekday): ScheduleSlot | null {
  const days = (slot.days ?? []).filter((d) => d !== day);
  return days.some((d) => Number.isInteger(d) && d >= 0 && d <= 6)
    ? { ...slot, days }
    : null;
}

/**
 * Sett en ny plassering på en opptreden. Slot med bare denne dagen skrives
 * på plass (og nullstiller `max`: opptaket varer til stopptiden); en slot med FLERE dager deles: den andre
 * dagene står, og denne dagen blir en egen slot sist i lista. Så rører
 * visningen aldri dager den ikke viser.
 *
 * Returnerer den nye lista og navnet på opptredenen etter endringen.
 */
export function replaceOccurrence(
  slots: readonly ScheduleSlot[],
  ref: OccurrenceRef,
  next: Placement,
): { slots: ScheduleSlot[]; ref: OccurrenceRef } {
  const slot = slotAt(slots, ref);
  if (!slot) return { slots: [...slots], ref };
  const times = slotTimes(next);
  const rest = withoutDay(slot, ref.day);
  if (rest === null) {
    const out = [...slots];
    out[ref.index] = { ...slot, days: [next.day], ...times, max: null };
    return { slots: out, ref: { index: ref.index, day: next.day } };
  }
  const out = [...slots];
  out[ref.index] = rest;
  out.push({ ...slot, days: [next.day], ...times, max: null });
  return { slots: out, ref: { index: out.length - 1, day: next.day } };
}

/** Fjern en opptreden. En slot med flere dager mister bare denne dagen. */
export function deleteOccurrence(
  slots: readonly ScheduleSlot[],
  ref: OccurrenceRef,
): ScheduleSlot[] {
  const slot = slotAt(slots, ref);
  if (!slot) return [...slots];
  const rest = withoutDay(slot, ref.day);
  if (rest === null) return withoutIndex(slots, ref.index);
  const out = [...slots];
  out[ref.index] = rest;
  return out;
}

/** Legg til en ny slot (`withSlot` er regelen for hvordan en tid blir en slot). */
export function appendPlacement(
  slots: readonly ScheduleSlot[],
  p: Placement,
): { slots: ScheduleSlot[]; ref: OccurrenceRef } {
  const out = withSlot(slots, p.day, timeOfDay(p.start), p.duration);
  return { slots: out, ref: { index: out.length - 1, day: p.day } };
}

/**
 * Et ledig sted til «Legg til»-knappen: standardtiden (søndag 11:00, 90 min)
 * hvis den er ledig, ellers første ledige kvarter-slott den finner. `null`
 * når uka ikke har plass til 90 minutter noe sted.
 */
export function suggestPlacement(
  occs: readonly Occurrence[],
  preferred: Placement,
): Placement | null {
  if (!findConflict(occs, preferred)) return preferred;
  for (let abs = 0; abs < WEEK_MINUTES; abs += SNAP) {
    const day = Math.floor(abs / DAY_MINUTES) as Weekday;
    const cand = {
      day,
      start: abs % DAY_MINUTES,
      duration: preferred.duration,
    };
    if (!findConflict(occs, cand)) return cand;
  }
  return null;
}

// ── Neste, savnet, nå ───────────────────────────────────────────────────────

/** Dag og klokkeslett (minutter) for en lokal-ISO fra planleggeren. */
function dayAndMinutes(
  at: string | null | undefined,
): { day: Weekday; minutes: number } | null {
  if (!at) return null;
  const ms = new Date(at).getTime();
  if (!Number.isFinite(ms)) return null;
  const d = new Date(ms);
  return { day: weekdayOf(d), minutes: minutesOfDate(d) };
}

export function occurrenceKey(day: number, minutes: number): string {
  return `${day}-${minutes}`;
}

/** Nøkkelen til opptredenen planleggeren sier starter neste gang. */
export function nextKey(at: string | null | undefined): string | null {
  const parts = dayAndMinutes(at);
  return parts ? occurrenceKey(parts.day, parts.minutes) : null;
}

/** Mandag 00:00 i uka `now` ligger i. */
export function weekStart(now: Date): Date {
  const d = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  d.setDate(d.getDate() - weekdayOf(now));
  return d;
}

/**
 * Nøklene til opptak som ikke ble tatt opp DENNE uka (mandag til søndag).
 * Eldre savnede varsler hører ikke hjemme på en blokk som gjentas hver uke.
 */
export function missedKeysThisWeek(
  missed: ReadonlyArray<{ at: string }>,
  now: Date,
): Set<string> {
  const from = weekStart(now).getTime();
  const to = new Date(
    new Date(from).getFullYear(),
    new Date(from).getMonth(),
    new Date(from).getDate() + 7,
  ).getTime();
  const out = new Set<string>();
  for (const m of missed) {
    const ms = new Date(m.at).getTime();
    if (!Number.isFinite(ms) || ms < from || ms >= to) continue;
    const parts = dayAndMinutes(m.at);
    if (parts) out.add(occurrenceKey(parts.day, parts.minutes));
  }
  return out;
}

/**
 * Hvor rutenettet står når siden åpnes (øverste synlige minutt).
 *
 * Standard er 06:00 øverst — natta er et rull unna. Er det en tid som ikke
 * ville vært i synsfeltet (tidligere enn 06:00, eller senere enn `visible`
 * minutter under toppen), flyttes toppen til en time over den første. Halen av
 * en tid som krysser midnatt teller ikke: den står alltid på 00:00 og ville
 * trukket visningen opp til natta for en tid som begynner kvelden før.
 */
export function initialTopMinutes(
  occs: readonly Occurrence[],
  visible: number = 9 * 60,
): number {
  const segs = segmentsOfAll(occs);
  const starts = segs.filter((s) => s.part !== "tail");
  const pool = starts.length > 0 ? starts : segs;
  if (pool.length === 0) return DEFAULT_TOP_MINUTES;
  const earliest = Math.min(...pool.map((s) => s.from));
  const inView =
    earliest >= DEFAULT_TOP_MINUTES &&
    earliest < DEFAULT_TOP_MINUTES + visible - 60;
  if (inView) return DEFAULT_TOP_MINUTES;
  const top = floorSnap(earliest - 60, 60);
  return clamp(top, 0, Math.max(0, DAY_MINUTES - visible));
}
