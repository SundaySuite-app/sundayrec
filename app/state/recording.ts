/**
 * «Går det et opptak akkurat nå?» — ett signal, én sannhet.
 *
 * Legacy-skallet har `window.__isRecording`: en mutabel global, skrevet av
 * handlere på de samme eventene som skriver `preroll-lifecycle`s egen
 * `recordingSeen` og `status/next-recording`s `state.isRecording`. Tre kopier
 * av det samme svaret, hver med sin handler, og rekkefølgen mellom dem er ikke
 * noe man skal være avhengig av når svaret avgjør HVEM SOM EIER MIKROFONEN.
 *
 * Her er det ett signal. `app/` gjenskaper ikke `window.__isRecording`.
 *
 * Kartleggingen fra hendelse til svar er `liveFromRecordingState` i
 * `@lib/preroll-lifecycle-core` — den samme rene funksjonen legacy bruker,
 * ikke en ny kopi. `null` derfra betyr «ingen mening»: en ukjent tilstand skal
 * la troen stå, ikke gjette. Å gjette «ikke opptak» slipper pre-roll løs midt i
 * en gudstjeneste; å gjette «opptak» holder den nede for alltid.
 */

import { signal } from "@preact/signals";
import { liveFromRecordingState } from "@lib/preroll-lifecycle-core";

/** Går det et opptak? */
export const isRecording = signal(false);

let dispose: (() => void) | null = null;

/**
 * Abonner på opptaks-eventene. Idempotent — et andre kall gir den samme
 * opprydderen i stedet for et andre sett lyttere på de samme kanalene.
 */
export function initRecording(): () => void {
  if (dispose) return dispose;

  const offs: Array<(() => void) | undefined> = [
    window.api.on("recording-overlay-start", () => {
      isRecording.value = true;
    }),
    // Kartlagt til `recording://state`, som fyrer på HVER overgang — les
    // tilstanden, ikke anta «stopp».
    window.api.on("recording-overlay-stop", (data: unknown) => {
      const live = liveFromRecordingState(
        (data as { state?: string } | undefined)?.state,
      );
      if (live !== null) isRecording.value = live;
    }),
    window.api.on("recording-finished", () => {
      isRecording.value = false;
    }),
    window.api.on("recording-error", () => {
      isRecording.value = false;
    }),
  ];

  dispose = () => {
    for (const off of offs) off?.();
    dispose = null;
  };
  return dispose;
}
