/**
 * Lagringsbeslutningene bak `app/state/settings.ts` — uten timer, uten IPC,
 * uten signaler.
 *
 * Legacy-utgaven (`legacy/renderer/state.ts`) er ~30 linjer der HELE logikken
 * bor inne i en `setTimeout`-lukking: fire moduleniv-variabler, en delt
 * promise, og en `flush` som må ta ned nøyaktig de samme fire. Den kan ikke
 * enhetstestes — node-gaten har ingen DOM, og selv med en ville testen målt
 * klokka i stedet for beslutningen.
 *
 * Det som faktisk er verdt å teste er tre ting, og alle tre er rene:
 *
 *   1. NÅR en skrivning skal skje (etterslepende debounce)
 *   2. Hvor mange IPC-rundturer en byge av endringer blir til (koalesering)
 *   3. HVA som sendes — hele innstillingsobjektet, aldri et utvalg
 *
 * Punkt 3 er R4-invarianten, og den er ikke en stilpreferanse: `settings_save`
 * tar imot hele vokabularet og skriver det som én rad. Et kuratert utvalg
 * betyr at et felt som ikke ble sendt blir stille satt tilbake til sin default
 * i basen — feilfamilien #113/#115, der en innstilling «ikke ville lagre seg»
 * og ingen kunne se hvorfor. `e2e/settings-seam.spec.ts` pinner det samme
 * utenfra, på `settingsSavePayloads`-sømmen.
 */

/** Den etterslepende timeren, som en verdi i stedet for en lukking. */
export interface SaveTimerState {
  /** Når den ventende skrivningen skal skje, eller `null` for «ingen venter». */
  dueAtMs: number | null;
}

/** Ingenting venter. */
export const IDLE_SAVE_TIMER: SaveTimerState = Object.freeze({ dueAtMs: null });

export type SaveAction =
  /** Ingen timer gikk fra før — start en. */
  | "arm"
  /** En timer gikk allerede — skyv den ut. Dette ER koaleseringen. */
  | "coalesce";

export interface SavePlan {
  action: SaveAction;
  next: SaveTimerState;
}

/**
 * Planlegg én lagringsforespørsel.
 *
 * Etterslepende, ikke ledende: den siste endringen i en byge er den som teller,
 * og en bruker som drar i en glidebryter skal ikke produsere 40 skrivninger.
 * `coalesce` skyver forfallet ut på nytt — det er derfor tre endringer i samme
 * åndedrag blir én rundtur, og hvorfor `flush` finnes for de gangene man ikke
 * kan vente.
 */
export function planSave(
  state: SaveTimerState,
  nowMs: number,
  delayMs: number,
): SavePlan {
  return {
    action: state.dueAtMs === null ? "arm" : "coalesce",
    next: { dueAtMs: nowMs + Math.max(0, delayMs) },
  };
}

/** Er den ventende skrivningen forfalt? */
export function isDue(state: SaveTimerState, nowMs: number): boolean {
  return state.dueAtMs !== null && nowMs >= state.dueAtMs;
}

export interface FlushPlan {
  /** `send` = skriv nå; `none` = det var ingenting som ventet. */
  action: "send" | "none";
  next: SaveTimerState;
}

/**
 * Planlegg en tvungen tømming — før navigasjon, før avslutning.
 *
 * `none` når ingenting venter er ikke en detalj: en `flush` som skrev uansett
 * ville sendt hele innstillingsobjektet på hver sidebytte, og gjort hver
 * navigasjon til en disk-skrivning.
 */
export function planFlush(state: SaveTimerState): FlushPlan {
  return {
    action: state.dueAtMs === null ? "none" : "send",
    next: { dueAtMs: null },
  };
}

/**
 * Det som krysser til backend.
 *
 * En grunn kopi, ikke et utvalg: hver eneste nøkkel blir med. Kopien finnes
 * bare for at en endring som skjer mens IPC-en er i lufta ikke skal endre
 * objektet mottakeren holder på — den fjerner ingenting.
 *
 * Denne funksjonen ser triviell ut, og det er nettopp derfor den er en
 * funksjon: den er det ene stedet noen ville lagt inn «send bare det som er
 * endret», og det ene stedet en test kan si at ingen har gjort det.
 */
export function payloadFor<T extends object>(settings: T): T {
  return { ...settings };
}
