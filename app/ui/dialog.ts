/**
 * Bekreftelsesdialogen — køen, ikke flaten.
 *
 * Samme signatur som `legacy/renderer/ui/dialog.ts`: `confirmDialog(opts)` gir
 * en `Promise<boolean>`. Og samme serialisering: to overlappende kall stabler
 * seg ikke oppå hverandre, den andre VENTER. En dialog som kommer i veien for
 * en annen dialog er hvordan folk klikker «Ja» på noe de aldri leste.
 *
 * Flaten (fokusfelle, Escape, `danger`-fargen, Enter som velger AVBRYT når det
 * er farlig) er S1b sin. Fram til den finnes er `activeDialog` et signal ingen
 * rendrer, og et `confirmDialog`-kall løses aldri — derfor injiseres `confirm`
 * i `useSetting` med denne som standard, i stedet for å være hardkodet der.
 */

import { signal } from "@preact/signals";

export interface ConfirmOpts {
  title: string;
  message?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Rød bekreft-knapp. Gjør også AVBRYT til Enter-valget — en destruktiv
   *  handling skal aldri være ett feiltrykk unna. */
  danger?: boolean;
}

export interface PendingDialog {
  id: number;
  spec: ConfirmOpts;
}

/** Dialogen som skal vises nå, eller `null`. Køen bak den er privat: en flate
 *  skal ikke kunne rendre nummer to før nummer én er besvart. */
export const activeDialog = signal<PendingDialog | null>(null);

interface QueueEntry extends PendingDialog {
  settle: (ok: boolean) => void;
}

const queue: QueueEntry[] = [];
let nextId = 1;

function publish(): void {
  const head = queue[0];
  activeDialog.value = head ? { id: head.id, spec: head.spec } : null;
}

/** Spør. Løses med `true` for bekreft, `false` for avbryt. */
export function confirmDialog(opts: ConfirmOpts): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    queue.push({ id: nextId++, spec: opts, settle: resolve });
    publish();
  });
}

/**
 * Svar på dialogen som står fremst. Det er dette S1b sin vert kaller.
 *
 * `id` er påkrevd og sjekkes: en vert som svarer på en dialog som allerede er
 * borte (en dobbeltklikk, en tastetrykk som kom etter) skal ikke kunne svare
 * på den NESTE i køen i stedet.
 */
export function resolveDialog(id: number, ok: boolean): void {
  const head = queue[0];
  if (!head || head.id !== id) return;
  queue.shift();
  publish();
  head.settle(ok);
}

/** Avbryt alt som venter. For teardown og tester. */
export function cancelAllDialogs(): void {
  const waiting = queue.splice(0, queue.length);
  publish();
  for (const entry of waiting) entry.settle(false);
}
