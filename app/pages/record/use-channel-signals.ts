/**
 * Hvilke kanaler på en flerkanals enhet som har signal — delt av lydarket og
 * første-gangs-skjermen (`SoundPage`).
 */

import { useEffect, useRef, useState } from "preact/hooks";

import { acquireVuFeed } from "@lib/audio/vu-feed";
import { nextSignalState } from "@lib/pages/channel-grid-logic";
import type { VuLevels } from "@legacy/bindings/VuLevels";

/**
 * Hvilke native kanaler som har signal NÅ.
 *
 * Et eget abonnement på den delte strømmen, ved siden av målerens. `acquireVuFeed`
 * er refcountet, så de to blir én økt på enheten — ikke to (`@lib/audio/vu-feed`).
 *
 * Tilstanden holdes i en `ref` og speiles til `state` bare når den ENDRER seg:
 * pakkene kommer 30 ganger i sekundet, og en `setState` per pakke ville rendret
 * hele siden 30 ganger i sekundet for å tenne en brikke som skifter noen ganger
 * i minuttet.
 *
 * `active` er `false` mens det TAS OPP, av nøyaktig samme grunn som `off` på
 * måleren og `active` i `use-vu-word.ts`: opptaksmotoren eier enheten, og en
 * `start_vu` derfra ville bedt om den midt i en gudstjeneste. Brikkene slukner
 * i stedet, som er sant — vi måler ingenting.
 */
export function useChannelSignals(
  deviceName: string | null,
  channels: number,
  active: boolean,
): boolean[] {
  const [lit, setLit] = useState<boolean[]>([]);
  const state = useRef<boolean[]>([]);

  useEffect(() => {
    state.current = [];
    setLit([]);
    if (!active || !deviceName || channels <= 2) return;
    const release = acquireVuFeed({
      deviceName,
      onLevels: (_l, _r, raw: VuLevels) => {
        const peaks = raw.peak_dbfs ?? [];
        let changed = false;
        const next = state.current.slice();
        for (let i = 0; i < channels; i++) {
          // RÅ verdi, ikke utjevnet: hysteresen på en dempet verdi ville holdt
          // en kanal «tent» like lenge som fallet varer etter at lyden sluttet.
          const on = nextSignalState(next[i] ?? false, peaks[i] ?? -120);
          if (on !== next[i]) {
            next[i] = on;
            changed = true;
          }
        }
        if (changed) {
          state.current = next;
          setLit(next);
        }
      },
    });
    return release;
  }, [deviceName, channels, active]);

  return lit;
}
