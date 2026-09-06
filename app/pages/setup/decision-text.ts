/**
 * Beslutningene som SETNINGER — det ene stedet `decisions-core`s data blir
 * tekst.
 *
 * Kjernen svarer med `{ key: "deviceMissing", name }`; her slås nøkkelen opp.
 * Skillet er hele grunnen kjernen er ren, og fila finnes fordi det er TO
 * flater som gjør oppslaget nå: nivå 1 og sjekklisten på siste skjerm i
 * første-gangs-sekvensen. To kopier av den samme oversettelsen ville før eller
 * siden sagt to forskjellige ting om nøyaktig samme tilstand — på to skjermer
 * en frivillig ser rett etter hverandre.
 */

import { t, tDyn, tf } from "../../i18n";
import type { Answer, DecisionId, Detail } from "./decisions-core";

/**
 * Spørsmålet hvert kort stiller.
 *
 * Fem literaler og ikke `tDyn('app.setup.q', n)`: nøklene heter `q1`…`q5` og
 * bor side om side med `lede`, `notSetUp` og resten, så et dynamisk oppslag
 * ville pekt på et subtre som er mye større enn de fem det gjelder — og da kan
 * `check-i18n-keys.mjs` ikke si noe om hvorvidt akkurat de fem finnes.
 */
export function questionText(id: DecisionId): string {
  switch (id) {
    case "sound":
      return t("app.setup.q1");
    case "folder":
      return t("app.setup.q2");
    case "quality":
      return t("app.setup.q3");
    case "church":
      return t("app.setup.q4");
    case "notify":
      return t("app.setup.q5");
  }
}

/** Svaret som står nå, som setning. */
export function answerText(answer: Answer): string {
  switch (answer.key) {
    case "notSetUp":
      return t("app.setup.notSetUp");
    case "device":
      return answer.pair
        ? tf("app.setup.sound.deviceWithPair", {
            name: answer.name,
            l: answer.pair.l,
            r: answer.pair.r,
          })
        : answer.name;
    case "deviceMissing":
      return tf("app.setup.sound.gone", { name: answer.name });
    case "path":
      return answer.path;
    case "quality":
      return tDyn("app.setup.quality", answer.format);
    case "qualityCustom":
      return tf("app.setup.qualityCustom", {
        format: answer.format,
        bitrate: answer.bitrate,
      });
    case "church":
      return answer.name;
    case "nobody":
      return t("app.setup.nobodyYet");
    case "email":
      return answer.address;
  }
}

/** Linja under svaret. `null` når det ikke er noe mer å si. */
export function detailText(detail: Detail | null): string | null {
  if (!detail) return null;
  switch (detail.key) {
    case "heard":
      return tDyn("app.vu", detail.word);
    case "deviceGone":
      return t("app.setup.sound.goneDesc");
    case "noDevice":
      return t("app.setup.sound.noneDesc");
    case "space": {
      // Hele GB og hele timer. «412,37 GB» og «299,8 t» er presist om et tall
      // ingen tar en beslutning på tredje desimal av.
      const gb = Math.round(detail.freeBytes / 1e9);
      if (detail.roomMinutes === null)
        return tf("app.setup.folder.free", { gb });
      return tf("app.setup.folder.space", {
        gb,
        hours: Math.floor(detail.roomMinutes / 60),
      });
    }
    case "noFolder":
      return t("app.setup.folder.noneDesc");
    case "qualityDesc":
      return tDyn("app.setup.qDesc", detail.format);
    case "qualityCustomDesc":
      return t("app.setup.qualityCustomDesc");
    case "language":
      return tf("app.setup.church.language", {
        language: tDyn("app.language", detail.language),
      });
    case "nobodyDesc":
      return t("app.setup.notify.nobodyDesc");
    case "emailDesc":
      return t("app.setup.notify.emailDesc");
  }
}
