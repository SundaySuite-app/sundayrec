/**
 * Avansert tidsplan — flere faste tider, og spesialopptak — som rene lister.
 *
 * ## Hva dette IKKE er
 *
 * Det er ikke en port av kalenderen. Legacy tegner en månedskalender med
 * helligdager, dagsdetaljer og et redigeringsskjema per dag (`calendar-page.ts`
 * + `#cal-grid`), og den er både stor og bygget for en annen skjerm. Her er de
 * samme DATAENE som to lister med en «Legg til»-rad, fordi det er den delen en
 * frivillig faktisk trenger: «det er konsert 12. desember klokka sju».
 *
 * ## Hvorfor listen er ren
 *
 * Begge listene endres ved INDEKS — «fjern den tredje» — og legacy identifiserer
 * dem på samme måte (`id` skrives alltid som `null` i `calendar-page.ts`, så det
 * dokumenterte «stabile id-en» finnes ikke i praksis). En indeks som gjelder en
 * sortert visning, men brukes mot den ULAGREDE rekkefølgen, sletter feil rad.
 * Det er nøyaktig den formen skjøtefeil har, så sorteringen og slettingen bor
 * her sammen, med tester.
 */

import type { ScheduleSlot } from "@legacy/bindings/ScheduleSlot";
import type { SpecialRecording } from "@legacy/bindings/SpecialRecording";

import { minutesOfDay, stopFor, type Weekday } from "../schedule-core";

/** Én rad i en liste som kan fjernes: verdien, og hvor den ligger LAGRET. */
export interface Row<T> {
  value: T;
  /** Indeksen i den lagrede listen — ikke i den sorterte visningen. */
  index: number;
}

/**
 * De faste tidene, i lagret rekkefølge, med indeksen sin.
 *
 * Ikke sortert: den FØRSTE sloten er den nivå 1 eier og viser, og en sortering
 * ville flyttet den rundt uten at noen forsto hvorfor «tiden min» plutselig
 * står nederst.
 */
export function slotRows(
  slots: readonly ScheduleSlot[] | null | undefined,
): Array<Row<ScheduleSlot>> {
  return (slots ?? []).map((value, index) => ({ value, index }));
}

/** Fjern raden på denne LAGREDE indeksen. */
export function withoutIndex<T>(
  list: readonly T[] | null | undefined,
  index: number,
): T[] {
  const all = [...(list ?? [])];
  if (index < 0 || index >= all.length) return all;
  all.splice(index, 1);
  return all;
}

/** Legg til en fast tid bakerst. Dagen er én dag — flere dager per slot er en
 *  form legacy tillater og ingen har brukt; her er én rad én dag. */
export function withSlot(
  slots: readonly ScheduleSlot[] | null | undefined,
  day: Weekday,
  start: string,
  minutes: number,
): ScheduleSlot[] {
  return [
    ...(slots ?? []),
    { days: [day], start, stop: stopFor(start, minutes), max: null },
  ];
}

/** Dagen en slot faktisk gjelder — den første valgte. `null` når den ikke har
 *  noen (en form fra en importert profil). */
export function slotDay(slot: ScheduleSlot): Weekday | null {
  const day = (slot.days ?? []).find((d) => d >= 0 && d <= 6);
  return day === undefined ? null : (day as Weekday);
}

/**
 * Spesialopptakene som skal VISES: framtidige først i datorekkefølge, med den
 * lagrede indeksen med seg.
 *
 * Passerte datoer faller ut av visningen, ikke ut av basen — bakenden luker
 * dem selv sju dager etter at de er over (`prune_specials`), og en liste som
 * slettet dem med én gang ville fjernet noe brukeren kan huske å ha lagt inn.
 */
export function specialRows(
  specials: readonly SpecialRecording[] | null | undefined,
  today: string,
): Array<Row<SpecialRecording>> {
  return (specials ?? [])
    .map((value, index) => ({ value, index }))
    .filter((row) => (row.value.date ?? "") >= today)
    .sort((a, b) =>
      `${a.value.date}T${a.value.start}` < `${b.value.date}T${b.value.start}`
        ? -1
        : 1,
    );
}

/** `YYYY-MM-DD` for en dato, i LOKAL tid. `toISOString()` er UTC og ville gjort
 *  «i dag» til «i går» hver kveld vest for Greenwich. */
export function isoDate(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

/** Det et nytt spesialopptak trenger. */
export interface SpecialDraft {
  name: string;
  /** `YYYY-MM-DD`. */
  date: string;
  /** `HH:MM`. */
  start: string;
  minutes: number;
}

/** Hvorfor et utkast ikke kan legges til, eller `null`. */
export type SpecialIssue = "noDate" | "badTime" | "past";

/**
 * Er utkastet noe bakenden kan planlegge?
 *
 * `past` er en ADVARSEL i legacy og en avvisning her: et opptak på en dato som
 * er passert starter aldri, og en rad i lista som aldri kommer til å skje er
 * en løgn med et klokkeslett på.
 */
export function checkSpecial(
  draft: SpecialDraft,
  today: string,
): SpecialIssue | null {
  if (!draft.date) return "noDate";
  if (minutesOfDay(draft.start) === null) return "badTime";
  if (draft.date < today) return "past";
  return null;
}

/** Legg til utkastet bakerst. `id` er `null`, som i legacy — ingenting
 *  genererer en, og listen identifiseres ved indeks. */
export function withSpecial(
  specials: readonly SpecialRecording[] | null | undefined,
  draft: SpecialDraft,
  fallbackName: string,
): SpecialRecording[] {
  return [
    ...(specials ?? []),
    {
      id: null,
      date: draft.date,
      name: draft.name.trim() || fallbackName,
      start: draft.start,
      stop: stopFor(draft.start, draft.minutes),
      deviceId: null,
    },
  ];
}

/** Hva `wake_capabilities` betyr for den ene setningen raden viser. */
export type WakeWord = "can" | "cannot" | "needsAdmin" | "unknown";

export interface WakeFacts {
  canWakeFromSleep: boolean;
  needsAdmin: boolean;
}

/**
 * Én setning, ikke en diagnostikkskjerm.
 *
 * Legacy har et helt kort med strøm, standby, verifisering og testhistorikk.
 * Det som avgjør om det planlagte opptaket skjer er det ene: kan maskinen
 * vekkes, og koster det et administratorpassord?
 */
export function wakeWord(facts: WakeFacts | null): WakeWord {
  if (facts === null) return "unknown";
  if (!facts.canWakeFromSleep) return "cannot";
  return facts.needsAdmin ? "needsAdmin" : "can";
}

/** Hva `wake_reschedule` betyr for setningen under «Aktiver vekking». */
export type WakeArmWord =
  /** Ikke forsøkt ennå — raden sier hva knappen kommer til å gjøre. */
  | "idle"
  | "ok"
  | "needsAdmin"
  | "disabled"
  | "unsupported"
  | "cancelled"
  | "failed";

/**
 * Ett ord av `WakeResult`.
 *
 * Bakendens `reason` er `disabled | cancelled | permission | unsupported |
 * error` (`src-tauri/src/wake/mod.rs`), og de betyr forskjellige ting for den
 * som står der: «trenger administratorpassord» er noe man kan GJØRE noe med,
 * «denne maskinen kan ikke vekkes» er det ikke. Å vise dem som én «det gikk
 * galt» ville kastet nettopp den forskjellen.
 *
 * En ukjent `reason` faller til `failed`: en feil vi ikke har et ord for er
 * fortsatt en feil, og aldri «det gikk bra». Ren funksjon, så tabellen står
 * ett sted.
 */
export function wakeArmWord(
  result: { ok: boolean; reason: string | null } | null,
): WakeArmWord {
  if (result === null) return "idle";
  if (result.ok) return "ok";
  switch (result.reason) {
    case "permission":
      return "needsAdmin";
    case "disabled":
      return "disabled";
    case "unsupported":
      return "unsupported";
    case "cancelled":
      return "cancelled";
    default:
      return "failed";
  }
}
