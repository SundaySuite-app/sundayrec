/**
 * «Resultat: 28 min 10 s (av 1 t 2 min)» og «Tale» — de to fakta som beskriver
 * det man er i ferd med å lage.
 *
 * De sto som en lokal `Summary` i `EditorPage.tsx` mens EKSPORTER var steg 3
 * der. Etter D3 er eksporten en egen destinasjon med sin egen topplinje, og
 * begge flatene sier fortsatt det samme om den samme fila: hvor mye som blir
 * igjen, og hvilken behandling som følger med.
 *
 * Setningen bygges ETT sted, av samme grunn som alt annet i dette skallet: to
 * steder som regner ut det samme tallet er to steder som kan bli uenige om
 * det, og da er det brukeren som oppdager forskjellen.
 */

import { tDyn, tf } from "../i18n";
import { exactSpan, keptSeconds } from "./editor-core";
import { cuts, duration } from "./model";
import { soundProfile } from "./sound";
import { spanLabel } from "./span";

/** «Resultat: X (av Y)» — hva som blir igjen etter kuttene. */
export function resultLine(): string {
  const total = duration.value;
  return tf("app.editor.result", {
    kept: spanLabel(exactSpan(keptSeconds(cuts.value, total))),
    total: spanLabel(exactSpan(total)),
  });
}

/** Navnet på lydprofilen eksporten kommer til å bruke. */
export function profileLabel(): string {
  return tDyn("app.editor.profile", soundProfile.value);
}
