/**
 * «14 t» / «14 t 20 min» / «45 min» — formen fra `record-core`, oversatt.
 *
 * En egen liten fil fordi BÅDE opptakssiden og overlegget sier det samme om et
 * tidsrom, og fordi den er den ene i18n-halvdelen av en ellers ren kjerne.
 *
 * Tre `tf()`-nøkler og ingen `tn()`. Begrunnelsen var en gang pausen — en ny
 * tellende nøkkel ville krevd polske flertallsformer midt i den — men pausen
 * er over (F2-S6), og valget står på egne ben: «t» og «min» er INVARIANTE
 * forkortelser i alle sju språk, i hele tallområdet de faktisk vises for. En
 * flertallsgruppe her ville vært sju identiske par.
 *
 * ⚠️ Det er forkortelsene som bærer det, ikke tallet. Skrives noen av dem ut
 * som et ord («14 timer»), er dette en `tn()`-nøkkel samme dag — se de tre
 * tellingene S6 måtte bygge om.
 */

import { tf } from "../../i18n";
import type { Span } from "./record-core";

export function spanText(span: Span): string {
  switch (span.kind) {
    case "hours":
      return tf("app.span.hours", { h: span.hours });
    case "hoursMinutes":
      return tf("app.span.hoursMinutes", { h: span.hours, m: span.minutes });
    case "minutes":
      return tf("app.span.minutes", { m: span.minutes });
    case "none":
      return "";
  }
}
