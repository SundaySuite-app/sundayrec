/**
 * «Legg ut» — kanalen, lenken og beskrivelsesmalen, som ren logikk.
 *
 * SundayRec laster ikke opp noe (`docs/FRIVILLIG.md` lover det, og R1 tok
 * delings-klyngen ut). «Legg ut» er steget RUNDT overleveringen: fila er
 * eksportert, teksten står klar til å kopieres, og knappen åpner siden
 * menigheten laster opp på. Hvilken side det er, avgjør bakenden
 * (`sundayrec_core::publish::upload_page_url`) ut fra den lagrede
 * innstillingen — skallet sender aldri en adresse over IPC.
 *
 * Det som bor HER er det skjermen trenger for å si noe før knappen trykkes:
 * kanalens navn, om den egne lenken kommer til å bli godtatt, og
 * beskrivelsen malen blir til.
 */

import type { PublishTarget } from "@legacy/bindings/PublishTarget";

/** Kanalene, i den rekkefølgen Oppsett viser dem. */
export const PUBLISH_TARGETS: readonly PublishTarget[] = [
  "soundcloud",
  "youtube",
  "spotify",
  "custom",
  "none",
];

/**
 * Produktnavnet — det samme på alle sju språk, så det står her og ikke i
 * katalogen. `null` for `custom` og `none`: de heter noe på hvert språk, og
 * det hører hjemme i katalogen.
 */
export function channelName(target: PublishTarget): string | null {
  switch (target) {
    case "soundcloud":
      return "SoundCloud";
    case "youtube":
      return "YouTube";
    case "spotify":
      return "Spotify";
    default:
      return null;
  }
}

/** Hvorfor en egen lenke ikke blir åpnet, eller `null` når den blir det. */
export type CustomUrlProblem = "empty" | "notHttps" | "invalid";

/** Samme tak som `CUSTOM_URL_MAX_LEN` i kjernen. */
const CUSTOM_URL_MAX_LEN = 2048;

/**
 * Speilet av `custom_upload_url` i kjernen, med en GRUNN i stedet for `None`.
 *
 * Bakenden er dommeren — den prøver lenken på nytt hver gang den skal åpnes —
 * men Oppsett skal kunne si «den blir ikke godtatt» mens noen skriver den, og
 * ikke først når en frivillig trykker på knappen søndag. Reglene er de samme:
 * `https://` og ingenting annet, ingen `bruker@` foran vertsnavnet, én linje
 * uten mellomrom, og et vertsnavn som har et navn i seg.
 */
export function customUrlProblem(raw: string): CustomUrlProblem | null {
  const url = raw.trim();
  if (url === "") return "empty";
  if (url.length > CUSTOM_URL_MAX_LEN) return "invalid";
  if (/[\p{Cc}\p{White_Space}]/u.test(url)) return "invalid";
  const scheme = "https://";
  if (url.length <= scheme.length) {
    return url.toLowerCase() === scheme ? "invalid" : "notHttps";
  }
  if (url.slice(0, scheme.length).toLowerCase() !== scheme) return "notHttps";
  const authority = url.slice(scheme.length).split(/[/?#]/u)[0] ?? "";
  if (authority === "" || authority.includes("@") || authority.includes("\\"))
    return "invalid";
  const colon = authority.lastIndexOf(":");
  const port = colon >= 0 ? authority.slice(colon + 1) : "";
  const host =
    colon >= 0 && port !== "" && /^\d+$/u.test(port)
      ? authority.slice(0, colon)
      : authority;
  if (host.startsWith(".") || !/[\p{L}\p{N}]/u.test(host)) return "invalid";
  return null;
}

/** Det malen kan fylle inn. */
export interface DescriptionFields {
  title: string;
  speaker: string;
  /** `YYYY-MM-DD`, eller `null` når opptaksdagen er ukjent. */
  date: string | null;
  church: string;
  /** Språket datoen skrives på. */
  locale: string;
}

/**
 * Beskrivelsen malen i Oppsett blir til.
 *
 * Feltene er `{tittel}`, `{taler}`, `{dato}` og `{kirke}` — og de engelske
 * `{title}`, `{speaker}`, `{date}` og `{church}`, fordi en mal skrevet på et
 * annet språk enn norsk ellers ville måttet blande. Store og små bokstaver
 * teller ikke. Et felt vi ikke kjenner (`{bibeltekst}`) blir stående som det
 * står, så en skrivefeil synes i stedet for å forsvinne.
 *
 * Et tomt felt etterlater ikke et dobbelt mellomrom: mellomrom slås sammen på
 * hver linje, og linjene trimmes i enden. Linjeskift røres ikke.
 */
export function renderDescription(
  template: string,
  fields: DescriptionFields,
): string {
  const values: Record<string, string> = {
    tittel: fields.title.trim(),
    title: fields.title.trim(),
    taler: fields.speaker.trim(),
    speaker: fields.speaker.trim(),
    dato: longDate(fields.date, fields.locale),
    date: longDate(fields.date, fields.locale),
    kirke: fields.church.trim(),
    church: fields.church.trim(),
  };
  const filled = template.replace(
    /\{([\p{L}]+)\}/gu,
    (whole, name: string) => values[name.toLowerCase()] ?? whole,
  );
  return filled
    .split("\n")
    .map((line) => line.replace(/[ \t]{2,}/gu, " ").replace(/[ \t]+$/u, ""))
    .join("\n")
    .trim();
}

/** «27. september 2026» på norsk, «27 September 2026» på engelsk, … */
function longDate(iso: string | null, locale: string): string {
  if (!iso) return "";
  const m = /^(\d{4})-(\d{2})-(\d{2})$/u.exec(iso);
  if (!m) return "";
  // Lokal midnatt, ikke `new Date(iso)` — den leses som UTC og kan bli
  // gårsdagen vest for Greenwich.
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  try {
    return new Intl.DateTimeFormat(locale, {
      day: "numeric",
      month: "long",
      year: "numeric",
    }).format(d);
  } catch {
    return iso;
  }
}
