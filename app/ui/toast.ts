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

/** Høyeste antall toaster på skjermen samtidig. En feil er permanent, så uten
 *  tak vokser køen i et langt opptak til den dekker stopp-området. */
export const MAX_TOASTS = 5;

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
  const durationMs = opts.durationMs ?? DEFAULT_MS[kind];

  // Fold identical messages into the one already showing (a failing poll
  // otherwise stacks one permanent toast per tick). Toasts with an action are
  // never folded: each carries its own callback.
  if (!opts.action) {
    const dup = toasts
      .peek()
      .find((x) => x.kind === kind && x.msg === msg && !x.action);
    if (dup) {
      const prev = timers.get(dup.id);
      if (prev) {
        clearTimeout(prev);
        timers.delete(dup.id);
      }
      if (durationMs > 0 && dup.durationMs > 0) {
        timers.set(
          dup.id,
          setTimeout(() => dismissToast(dup.id), durationMs),
        );
      }
      return () => dismissToast(dup.id);
    }
  }

  const id = nextId++;
  let queue = [
    ...toasts.peek(),
    { id, kind, msg, action: opts.action, durationMs },
  ];
  // Over the cap: evict the oldest, but prefer one that is not an error —
  // the message you can least afford to miss is the one that stays.
  while (queue.length > MAX_TOASTS) {
    const victim = queue.find((x) => x.kind !== "error") ?? queue[0]!;
    const timer = timers.get(victim.id);
    if (timer) {
      clearTimeout(timer);
      timers.delete(victim.id);
    }
    queue = queue.filter((x) => x.id !== victim.id);
  }
  toasts.value = queue;
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
