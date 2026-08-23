/**
 * Sekunder ↔ piksler, og ingenting mer.
 *
 * ## Hvorfor dette er fire linjer og ikke 148
 *
 * Legacys `geometry.ts` deler lerretet i TRE regioner — intro, hovedopptak,
 * outro — og har en «utvidet tidslinje» der negative sekunder betyr «inne i
 * intro-jingelen». Alt det finnes for jinglene, og jingler er ikke med i
 * canvasens sett 4 i det hele tatt: 4.1 er bølgeformen, forslaget og
 * håndtakene. Så den utvidede tidslinja er ikke portet — den er ikke fjernet
 * fra appen (legacy-skallet har den fortsatt), den er ikke bygget ennå.
 *
 * Det som er igjen er de to funksjonene resten av editoren faktisk kaller, med
 * nøyaktig samme betydning som i legacy når intro og outro er null: `secToX`
 * og `xToSec` over utsnittet.
 *
 * Konsekvensen er at `clampPlayable` og `clampMain` er den SAMME funksjonen
 * her. Den heter `clampToFile`, én gang, i stedet for å bli to navn på det
 * samme som en dag rekker å bli uenige.
 */

import { E } from "./model";

/** Klem et sekundtall inn i opptaket. */
export function clampToFile(sec: number): number {
  return Math.max(0, Math.min(E.duration, sec));
}

/** Sekunder → x-piksel i lerretets CSS-bredde `W`. */
export function secToX(sec: number, W: number): number {
  const span = E.vpEnd - E.vpStart;
  if (span <= 0 || W <= 0) return 0;
  return ((sec - E.vpStart) / span) * W;
}

/** x-piksel → sekunder. Klemt til utsnittet: en x utenfor lerretet betyr
 *  kanten, ikke et sekundtall utenfor opptaket. */
export function xToSec(x: number, W: number): number {
  const span = E.vpEnd - E.vpStart;
  if (span <= 0 || W <= 0) return E.vpStart;
  if (x <= 0) return E.vpStart;
  if (x >= W) return E.vpEnd;
  return E.vpStart + (x / W) * span;
}

/**
 * Hvor nær en grense må være i sekunder for at et klikk skal regnes som «på
 * den». Ti piksler, oversatt til den zoomen som gjelder nå — slik at en
 * grense er like lett å treffe uansett hvor langt inn man har zoomet.
 */
export function grabThreshold(W: number): number {
  const span = E.vpEnd - E.vpStart;
  if (span <= 0 || W <= 0) return 0.1;
  return (span / W) * 10;
}
