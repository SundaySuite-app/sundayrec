/**
 * «14 t» / «14 t 20 min» / «45 min» — formen fra `record-core`, oversatt.
 *
 * En egen liten fil fordi BÅDE opptakssiden og overlegget sier det samme om et
 * tidsrom, og fordi den er den ene i18n-halvdelen av en ellers ren kjerne.
 *
 * Tre `tf()`-nøkler og ingen `tn()`: `check-i18n-plurals.mjs` krever hver
 * flertallsgruppe i ALLE sju språk med riktige CLDR-kategorier, og har ingen
 * unntak for de fem som er pauset. En ny tellende nøkkel ville altså krevd
 * polske flertallsformer midt i pausen som finnes for å slippe akkurat det.
 * Formene over er riktige for hele tallområdet de faktisk vises for.
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
