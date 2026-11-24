/**
 * Avansert tidsplan — flere faste tider — som en ren liste, og vekke-ordene.
 *
 * ## Hvorfor listen er ren
 *
 * Listen endres ved INDEKS — «fjern den tredje» — og legacy identifiserer den
 * på samme måte. En indeks som gjelder en sortert visning, men brukes mot den
 * ULAGREDE rekkefølgen, sletter feil rad. Det er nøyaktig den formen skjøtefeil
 * har, så slettingen bor her, med tester.
 */

import type { ScheduleSlot } from "@legacy/bindings/ScheduleSlot";

import { stopFor, type Weekday } from "../schedule-core";

/** Én rad i listen: verdien, og hvor den ligger LAGRET. */
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

/** `YYYY-MM-DD` for en dato, i LOKAL tid. `toISOString()` er UTC og ville gjort
 *  «i dag» til «i går» hver kveld vest for Greenwich. */
export function isoDate(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
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
  /** Lyktes, men armet INGENTING: «Ta opp automatisk» er av. */
  | "autoRecordOff"
  /** Lyktes, men armet ingenting: ingenting ligger innenfor horisonten. */
  | "nothingUpcoming"
  | "needsAdmin"
  | "disabled"
  | "unsupported"
  | "cancelled"
  | "failed";

/** Det `wakeArmWord` trenger av `WakeResult`. */
export interface WakeArmResult {
  ok: boolean;
  reason: string | null;
  /** Hvor mange vekkinger som faktisk ble registrert. */
  count?: number | null;
  /** Hvorfor et VELLYKKET forsøk armet null (`WakeIdleReason`). */
  idleReason?: string | null;
}

/**
 * Ett ord av `WakeResult`.
 *
 * Bakendens `reason` er `disabled | cancelled | permission | unsupported |
 * error` (`src-tauri/src/wake/mod.rs`), og de betyr forskjellige ting for den
 * som står der: «trenger administratorpassord» er noe man kan GJØRE noe med,
 * «denne maskinen kan ikke vekkes» er det ikke. Å vise dem som én «det gikk
 * galt» ville kastet nettopp den forskjellen.
 *
 * ⚠️ `ok: true` er heller ikke ETT svar. En vellykket runde som registrerte
 * NULL vekkinger er ikke «vekkingen er registrert» — det er en knapp som ser
 * ut som den virket. Bakenden sier hvilken ingenting det er (`idleReason`:
 * bryteren er av, eller ingenting ligger innenfor horisonten), og den
 * forskjellen er hele grunnen feltet finnes. Mangler den på et `count: 0`-svar
 * (en eldre payload — feltet er `serde(default)`), faller vi til den generelle
 * av de to, som er sann i begge tilfellene.
 *
 * En ukjent `reason` faller til `failed`: en feil vi ikke har et ord for er
 * fortsatt en feil, og aldri «det gikk bra». Ren funksjon, så tabellen står
 * ett sted.
 */
export function wakeArmWord(result: WakeArmResult | null): WakeArmWord {
  if (result === null) return "idle";
  if (result.ok) {
    if ((result.count ?? 0) > 0) return "ok";
    return result.idleReason === "autoRecordOff"
      ? "autoRecordOff"
      : "nothingUpcoming";
  }
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

/** Det `testWakeWord` trenger av `TestWakeResult` (W6). */
export interface TestWakeOutcome {
  ok: boolean;
  reason?: string | null;
}

/**
 * Hva `wake_test` betyr for setningen under «Test vekking» (W6).
 *
 * `"needsAdmin" | "unsupported" | "cancelled" | "failed"` er navngitt likt
 * `WakeArmWord` med vilje — samme fire utfall av samme `WakeErrorReason`-sett
 * (`src-tauri/src/wake/mod.rs`), så raden kan sitere `wakeArmWord`-nøklene i
 * katalogen i stedet for å skrive de samme fire setningene på nytt. `scheduled`
 * og `idle` er testens egne: `wake_test` har ikke `WakeArmWord`s «ok, men armet
 * null»-utfall — en vellykket test armer alltid nøyaktig én vekking, to
 * minutter fram, aldri null.
 */
export type TestWakeWord =
  "idle" | "scheduled" | "needsAdmin" | "unsupported" | "cancelled" | "failed";

export function testWakeWord(result: TestWakeOutcome | null): TestWakeWord {
  if (result === null) return "idle";
  if (result.ok) return "scheduled";
  switch (result.reason) {
    case "permission":
      return "needsAdmin";
    case "unsupported":
      return "unsupported";
    case "cancelled":
      return "cancelled";
    default:
      return "failed";
  }
}
