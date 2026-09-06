/**
 * Lydnivå i ORD — «Vi hører lyd», «Vi hører ingenting», «For høyt».
 *
 * ## Hvorfor ord og ikke tall
 *
 * Canvasens sett 0 låser det: nivå 1 viser aldri et tall. En frivillig som
 * ser «−18,4 dBFS» vet ikke om det er bra, og appen har ikke hjulpet henne
 * med noe. «Vi hører lyd» svarer på spørsmålet hun faktisk har. Tallene finnes
 * fortsatt — under Avansert og i opptaksoverlegget — for den som vil ha dem,
 * og `VuMeter` slår dem på med `showNumbers`.
 *
 * ## To terskler, og hvorfor akkurat de
 *
 * `HEARD_DB` = −50 dBFS er den samme grensen opptaksmotoren bruker for
 * «stillhet» (`silenceThreshold` sin standardverdi). Én terskel for hva
 * stillhet ER, ikke to som er nesten like.
 *
 * `LOUD_DB` = −3 dBFS er marginen mot digital klipping. Digitalt fullskala er
 * 0; et program som topper over −3 har ikke rom igjen for det ene skriket i
 * mikrofonen, og klipping kan ikke repareres etterpå.
 *
 * ## PEAK, ikke RMS
 *
 * «For høyt» handler om toppene: en preken med snitt −20 dB og topper på
 * −1 dB klipper, og RMS-en ville aldri sagt fra. `vu://levels` bærer begge
 * (`peak_dbfs` og `rms_dbfs`), så det er bare å velge riktig.
 */

import { VU_FLOOR_DB } from "@lib/audio/vu-feed-core";

/** Over dette hører vi noe. Samme grense som motorens stillhets-standard. */
export const HEARD_DB = -50;
/** Over dette er det for høyt — marginen mot klipping. */
export const LOUD_DB = -3;

export type LevelWord = "nothing" | "hear" | "loud";

/** Ordet for én toppverdi i dBFS. */
export function levelWord(peakDb: number): LevelWord {
  // En pakke uten tall (`null` → NaN et sted oppstrøms) er ikke bevis for
  // lyd. Stillhet er det ærlige svaret.
  if (!Number.isFinite(peakDb)) return "nothing";
  if (peakDb >= LOUD_DB) return "loud";
  if (peakDb > HEARD_DB) return "hear";
  return "nothing";
}

/** Ordet for et stereopar: den høyeste av de to bestemmer. */
export function levelWordFor(peakL: number, peakR: number): LevelWord {
  const a = Number.isFinite(peakL) ? peakL : VU_FLOOR_DB;
  const b = Number.isFinite(peakR) ? peakR : VU_FLOOR_DB;
  return levelWord(Math.max(a, b));
}

/**
 * dBFS → 0..1 for hvor langt stolpen skal fylles.
 *
 * Lineær i dB, ikke i amplitude: en amplitudeskala klemmer alt under −20 dB
 * inn i de nederste prosentene, og der ligger nesten all tale. Gulvet er
 * `VU_FLOOR_DB` (−60), det samme gulvet hver måler i appen deler.
 */
export function levelFraction(db: number): number {
  if (!Number.isFinite(db)) return 0;
  const clamped = Math.max(VU_FLOOR_DB, Math.min(0, db));
  return (clamped - VU_FLOOR_DB) / -VU_FLOOR_DB;
}

/**
 * De tre ordene, som en liste.
 *
 * Katalogen slås opp med `tDyn("app.vu", word)` på KALLSTEDET, ikke gjennom en
 * oppslagstabell her: `check-i18n-keys.mjs` krever at prefikset er en literal
 * for å kunne slå det opp, og en `Record<LevelWord, string>` med ferdige
 * nøkler er nøyaktig den formen gaten ikke kan se inn i. Listen finnes så
 * `app.vu`-subtreet og typen kan holdes i takt av en test.
 */
export const LEVEL_WORDS: readonly LevelWord[] = ["nothing", "hear", "loud"];
