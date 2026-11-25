/**
 * Varselet om en død kanal.
 *
 * Når et opptak åpnes spør vi Rust om den ene siden er stille mens den andre har lyd
 * (`editor_channel_check`; svaret kommer fra måleren åpne-runden uansett kjører, så det er
 * ingen ekstra lesing av fila). Svaret er en KODE, `left` eller `right`; skjermen eier ordene:
 * «Venstre side er stille — sjekk kabelen».
 *
 * Bare varselet. Reparasjonen (kopier den gode siden til begge) ble fjernet i F3, og et
 * stille valg å slå på hører ikke hjemme her: den frivillige skal vite at kabelen er feil, så
 * den kan rettes til neste gudstjeneste.
 */

import { signal } from "@preact/signals";

import { E } from "./model";

const SIDES = ["left", "right"] as const;
export type DeadSide = (typeof SIDES)[number];

/** Hvilken side som er død i fila som er åpen, eller `null`. */
export const deadChannel = signal<DeadSide | null>(null);

/** Nøkkelen under `app.editor.deadChannel` for en kode, eller `null` for alt vi ikke har ord for. */
export function deadChannelKey(
  side: string | null | undefined,
): DeadSide | null {
  return SIDES.find((s) => s === side) ?? null;
}

/**
 * Spør om kanalene til fila som står åpen. Feil og ukjente svar sier ingenting: et opptak
 * vi ikke fikk lest får ikke et varsel vi ikke kan stå inne for. Et svar som kommer etter at
 * en annen fil er åpnet blir kastet.
 */
export async function checkChannels(token: string): Promise<void> {
  try {
    const res = await window.api.editorChannelCheck(token);
    if (E.sourceToken === token)
      deadChannel.value = deadChannelKey(res?.deadSide);
  } catch {
    /* ingen varsel er et gyldig svar */
  }
}

export function resetChannelCheck(): void {
  deadChannel.value = null;
}
