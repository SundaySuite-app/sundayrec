/**
 * «45 s» / «28 min 10 s» / «1 t 2 min» — den ene i18n-halvdelen av `exactSpan`.
 *
 * En egen liten fil, av samme grunn som `pages/record/span-text.ts`: TO flater
 * sier det samme om det samme tidsrommet — resultatlinja og gullvinduets
 * etikett — og et tall som er avrundet på den ene og presist på den andre
 * betyr «Preken · 4 min» rett under «… — 3 min 30 s». To tall om det samme,
 * samtidig, på samme skjerm.
 *
 * Tre `tf()`-nøkler og ingen `tn()`. Pausen var én grunn til det (F2-S6
 * avsluttet den); den som består er at «s», «min» og «t» er INVARIANTE
 * forkortelser i alle sju språk, i hele tallområdet de faktisk vises for.
 */

import { tf } from "../i18n";
import type { ExactSpan } from "./editor-core";

export function spanLabel(span: ExactSpan): string {
  switch (span.kind) {
    case "seconds":
      return tf("app.editor.seconds", { n: span.seconds });
    case "minutesSeconds":
      return tf("app.editor.minutesSeconds", {
        m: span.minutes,
        s: span.seconds,
      });
    case "hoursMinutes":
      return tf("app.span.hoursMinutes", { h: span.hours, m: span.minutes });
  }
}
