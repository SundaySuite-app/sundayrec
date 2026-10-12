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
 *   3. HVA som sendes — de endrede feltene (`patchFor`), som Rust fletter over
 *      det lagrede
 *
 * Punkt 3 var tidligere «hele objektet, aldri et utvalg» (R4), fordi
 * `settings_save` erstattet raden og et felt som ikke ble sendt ble stille satt
 * tilbake til default (feilfamilien #113/#115). Nå fletter Rust, så det er
 * motsatt som er faren: å sende alt lar en utdatert kopi overskrive nyere
 * verdier. `e2e/settings-seam.spec.ts` pinner utfallet utenfra, på basen.
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
 * Det som krysser til backend: feltene i `current` som skiller seg fra
 * `baseline` (det basen sist svarte, med våre egne skrivinger lagt oppå).
 *
 * Rust fletter dette over det som ligger lagret (`settings::save_patch_from_renderer`),
 * så felt vi ikke nevner beholder verdien de har i basen. Det er hele poenget:
 * en kopi som er eldre enn basen (planleggeren ryddet et spesialopptak, en
 * profil ble importert) kan ikke lenger skrive sine gamle verdier over felt den
 * aldri rørte. Motsatt av den gamle R4-regelen «send alt» — den var til for at
 * et utvalg ikke skulle sette uspesifiserte felt tilbake til default, og det
 * kan ikke skje når mottakeren fletter i stedet for å erstatte (#113/#115).
 *
 * Et felt `baseline` ikke kjenner (nytt i en nyere bakende) regnes som endret.
 * Likhet er JSON-likhet; et objekt med annen nøkkelrekkefølge gir i verste fall
 * ett felt for mye, aldri ett for lite.
 */
export function patchFor<T extends object>(
  current: T,
  baseline: Partial<T>,
): Partial<T> {
  const patch: Partial<T> = {};
  for (const key of Object.keys(current) as Array<keyof T & string>) {
    const was = baseline[key];
    const now = current[key];
    if (was === now) continue;
    if (JSON.stringify(was) === JSON.stringify(now)) continue;
    patch[key] = now;
  }
  return patch;
}
