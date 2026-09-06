/**
 * Toast-køen — modellen, ikke flaten.
 *
 * Samme signatur som `legacy/renderer/ui/toast.ts` (`toast(kind, msg, opts)`
 * som returnerer en avvis-funksjon), men her er det bare en liste i et signal.
 * Legacy-utgaven bygger DOM: `document.createElement`, en stabel den fester på
 * `document.body`, klasser den setter og fjerner på timere. I et Preact-tre er
 * det feil sted å ta beslutningen — flaten hører hjemme i en komponent, og
 * komponenten kommer i S1b.
 *
 * Splittet er også det som gjør at `useSetting` kan testes: kjernen kaller
 * `toast()` på ekte, og en test kan lese `toasts.value` i stedet for å måtte ha
 * et dokument.
 *
 * S1b monterer verten som rendrer `toasts`; fram til da er en toast
 * usynlig, men den er der og telles.
 */

import { signal } from "@preact/signals";

export type ToastKind = "info" | "success" | "warn" | "error";

export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface ToastOpts {
  /** Én innebygd handling, f.eks. «Vis mappe». */
  action?: ToastAction;
  /** Overstyr tiden. `0` = blir stående. */
  durationMs?: number;
}

export interface ToastItem {
  id: number;
  kind: ToastKind;
  msg: string;
  action?: ToastAction;
  /** `0` betyr «forsvinner ikke av seg selv». */
  durationMs: number;
}

/**
 * Standardtider, kopiert fra legacy-utgaven — inkludert den ene som betyr
 * noe: en FEIL forsvinner ikke av seg selv. Den ene meldingen du ikke har råd
 * til å gå glipp av skal ikke være den som forsvinner mens du ser en annen vei.
 */
export const DEFAULT_MS: Record<ToastKind, number> = {
  info: 3200,
  success: 2600,
  warn: 5000,
  error: 0,
};

/** Køen slik den er nå, eldst først. */
export const toasts = signal<readonly ToastItem[]>([]);

let nextId = 1;
const timers = new Map<number, ReturnType<typeof setTimeout>>();

/** Fjern én toast (idempotent). */
export function dismissToast(id: number): void {
  const timer = timers.get(id);
  if (timer) {
    clearTimeout(timer);
    timers.delete(id);
  }
  const before = toasts.peek();
  const after = before.filter((x) => x.id !== id);
  if (after.length !== before.length) toasts.value = after;
}

/** Vis en melding. Returnerer en funksjon som fjerner den igjen. */
export function toast(
  kind: ToastKind,
  msg: string,
  opts: ToastOpts = {},
): () => void {
  const id = nextId++;
  const durationMs = opts.durationMs ?? DEFAULT_MS[kind];
  toasts.value = [
    ...toasts.peek(),
    { id, kind, msg, action: opts.action, durationMs },
  ];
  if (durationMs > 0) {
    timers.set(
      id,
      setTimeout(() => dismissToast(id), durationMs),
    );
  }
  return () => dismissToast(id);
}

/** Tøm køen. For teardown og tester — aldri som svar på noe brukeren gjorde. */
export function clearToasts(): void {
  for (const timer of timers.values()) clearTimeout(timer);
  timers.clear();
  toasts.value = [];
}
