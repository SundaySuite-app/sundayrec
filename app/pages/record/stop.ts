/**
 * Å stoppe opptaket — holdt inne, ikke bekreftet.
 *
 * ## «Hold» erstatter dialogen
 *
 * Før spurte `confirmAndStop` «Stoppe opptaket?» med «Fortsett» som
 * primærknapp. Den nye opptaksmodusen (E6) har én kontroll for det: den røde
 * knappen som må holdes i ett sekund (`HoldStop`). Et uhell midt i prekenen
 * koster fortsatt mer enn ett trykk — det koster ett HELT sekund med
 * fingeren på — men ingen dialog skal besvares mens lyden går.
 *
 * Det som holdt dialogen trygg gjelder like mye nå, og bor her:
 *
 *   - `beginStop()` sier nei mens motoren allerede skriver ferdig (en andre
 *     stoppforespørsel er ingen ny stoppforespørsel) og leser øktas identitet
 *     (`sessionStartedAtMs`) i det holdet BEGYNNER.
 *   - `completeStop()` sjekker den identiteten på nytt når sekundet er omme. Et
 *     hold kan vare lenge nok til at økta tok slutt av seg selv (planlagt stopp,
 *     full disk, tapt enhet) — eller tok slutt og en NY startet. «Stopp» betyr
 *     da ikke lenger det hun holdt inne for å stoppe.
 *   - `enterFinalizing()` FØR kommandoen, som alltid: skjermen sier «Fullfører»
 *     fra første øyeblikk, og de neste trykkene ignoreres.
 *
 * ## Menylinjens «Stopp opptak»
 *
 * Er et bevisst valg i en meny, ikke en knapp man kan treffe ved et uhell, og
 * ber ikke om noe hold: `stopNow()` går gjennom de samme vaktene.
 */

import {
  endSessionLocally,
  enterFinalizing,
  finalizing,
  isRecording,
  sessionStartedAtMs,
} from "../../state/recording";

/** Hvor lenge knappen holdes inne (ms). Ringen i `HoldStop` fyller seg over det samme. */
export const HOLD_TO_STOP_MS = 1000;

/** Øktas identitet slik den var da holdet begynte. */
export interface StopTicket {
  startedAt: number | null;
}

/** Begynn et stopp. `null` = ignorer: motoren skriver allerede ferdig, eller
 *  det går ikke noe opptak. */
export function beginStop(): StopTicket | null {
  if (finalizing.peek() || !isRecording.peek()) return null;
  return { startedAt: sessionStartedAtMs.peek() };
}

/** Fullfør stoppet — men bare hvis det fortsatt er DEN økta. */
export async function completeStop(ticket: StopTicket | null): Promise<void> {
  if (ticket === null) return;
  if (
    !isRecording.peek() ||
    finalizing.peek() ||
    sessionStartedAtMs.peek() !== ticket.startedAt
  ) {
    return;
  }

  enterFinalizing();
  try {
    await window.api.stopRecordingNow();
  } catch (err) {
    // Selve forespørselen feilet, så det kommer ingen terminal hendelse —
    // rydd her i stedet for å vente ut de 30 sekundene.
    console.error("[record] stop_recording feilet:", err);
    endSessionLocally();
  }
}

/** Stopp nå, uten hold (menylinja). Samme vakter. */
export function stopNow(): Promise<void> {
  return completeStop(beginStop());
}
