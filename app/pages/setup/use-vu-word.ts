/**
 * «Hører vi lyd?» som ETT ord, uten en måler på skjermen.
 *
 * Nivå 1 viser ikke stolper — det er spørsmål 1 sin underside som gjør det —
 * men kortet skal kunne si «✓ Vi hører lyd» i stedet for bare å gjenta
 * enhetsnavnet. Hooken er derfor en `VuMeter` uten tegningen: samme delte
 * bakenden-strøm, samme terskler, bare ordet ut.
 *
 * ## Den ene grunnen den kan slås av
 *
 * `active` er `false` mens det TAS OPP. Rust stopper VU-strømmen når
 * `start_recording` åpner enheten, og en `start_vu` som kommer etterpå ville
 * bedt om den samme enheten opptaket holder. Rust eier enheten og ville sagt
 * nei — men å be om det i det hele tatt, midt i en gudstjeneste, er ikke noe
 * en innstillingsskjerm skal gjøre for å pynte på en linje tekst.
 *
 * Ingen egen `getUserMedia`: `acquireVuFeed` er refcountet, så måleren på
 * undersiden og denne hooken deler én økt (se `@lib/audio/vu-feed`).
 */

import { useEffect, useRef, useState } from "preact/hooks";

import { acquireVuFeed } from "@lib/audio/vu-feed";
import { pickLR } from "@lib/audio/vu-feed-core";
import type { VuLevels } from "@lib/../bindings/VuLevels";

import { levelWordFor, type LevelWord } from "../../audio/level-words";

/** Ordet måleren ville sagt akkurat nå, eller `null` når ingen lytter. */
export function useVuWord(
  deviceName: string | null | undefined,
  active: boolean,
): LevelWord | null {
  const [word, setWord] = useState<LevelWord | null>(null);
  const last = useRef<LevelWord | null>(null);

  useEffect(() => {
    if (!active) {
      last.current = null;
      setWord(null);
      return;
    }
    const release = acquireVuFeed({
      deviceName,
      onLevels: (_l, _r, raw: VuLevels) => {
        // PEAK og ikke RMS: «for høyt» handler om toppene (se level-words.ts).
        const peak = pickLR(raw.peak_dbfs, "stereo", 0, 1);
        const next = levelWordFor(peak.l, peak.r);
        // Bare når ordet FAKTISK endrer seg — ellers ville 30 pakker i
        // sekundet blitt 30 re-render av hele nivå 1.
        if (next !== last.current) {
          last.current = next;
          setWord(next);
        }
      },
    });
    return () => {
      release();
      last.current = null;
    };
  }, [deviceName, active]);

  return word;
}
