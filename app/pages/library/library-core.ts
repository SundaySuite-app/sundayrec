/**
 * Biblioteksidens avgjørelser — en tabell, ikke betingelser inne i en
 * JSX-linje. Samme grense som `record-core.ts` trekker: alt som er ren
 * aritetikk står her, alt som er avhengig av `Intl` (ukedag, dato, klokke) står
 * det IKKE, fordi en tabell ikke skal bli skjør av hvilken ICU-versjon node
 * eller WebKit er bygget med.
 *
 * ## Raden er en ØKT, ikke en fil
 *
 * Et opptak med kamera skriver TO historikkrader — `{stem}.{video_ext}` og
 * lyd-sidevognen `{stem}.{audio_ext}` i samme mappe. `pairRecordings` i
 * `@lib/pages/history-core` folder dem til én rad på den delte grunnstien, og
 * den avgjørelsen GJENBRUKES her i stedet for å skrives på nytt: kommentaren
 * over den forklarer hvorfor nabolagsheuristikken den erstattet ikke kunne
 * virke under Tauri-shimmen, og to utgaver av den regelen ville vært to steder
 * å ta feil.
 *
 * ## Datoen er `startedAt`, ikke `timestamp`
 *
 * `timestamp` er `created_at ?? started_at` — når RADEN ble skrevet, altså når
 * gudstjenesten var FERDIG. Canvasens 3.1 setter klokkeslettet i radens
 * tittel («Søndag 16. august 2026 · 11:00»), og der er «12:05» ikke en
 * unøyaktighet, det er feil tid. Shimmen bærer derfor `startedAt` videre (P3,
 * additivt), og den er det raden dateres etter. Faller den bort, faller vi
 * tilbake på `timestamp` — en rad uten dato i det hele tatt er verre.
 *
 * ## Brikkene som IKKE finnes
 *
 * Canvasens 3.1 har fire: «Video», «Eksportert», «Redigert» og «Avbrutt»,
 * pluss «manuelt» som et dempet tillegg i tittelen. Bare den første har en
 * kilde. `recordings_list`-raden er `id, file_path, device_name, started_at,
 * duration_ms, byte_size, created_at, note` og ikke noe mer, og api-shimmens
 * `rowToEntry` setter `status: "ok"` KONSTANT («recordings_list only holds
 * completed recordings»). Det finnes altså ingenting å lese for «Avbrutt», og
 * ingenting som skiller et manuelt opptak fra et planlagt. Et merke som
 * gjettes er verre enn ingen merke — samme regel som P2 skrev ned for
 * «Redigert» og «Eksportert».
 *
 * ## Varigheten: 0 er UKJENT
 *
 * `rowToEntry` gjør en manglende `duration_ms` til `durationSec: 0`, så 0 er
 * tvetydig — enten et opptak uten lyd, eller en rad som aldri fikk en
 * varighet. WKWebView-proben i P2 fant nøyaktig den setningen på eierens egen
 * maskin («Lørdag 8. august · 0 min»). Her betyr 0 derfor `null`, og en rad
 * uten varighet sier «—» i stedet for å påstå noe.
 */

import { pairRecordings } from "@lib/pages/history-core";
import type { RecordingEntry } from "@lib/../types";

/** Grensen søket slår inn på. Legacys egen: under to tegn filtreres ingenting,
 *  fordi «a» ville skjult nesten hele arkivet ved første tastetrykk. */
export const MIN_QUERY_LENGTH = 2;

/** Én rad i Bibliotek: en økt, med filene den består av. */
export interface LibraryRow {
  /** Stabil nøkkel for lista. Stien er unik per opptak; en rad uten sti får
   *  sin egen plass i stedet for å kollidere med de andre stiløse. */
  key: string;
  /** Radens eget opptak — lyd-halvdelen når økta har to filer. */
  entry: RecordingEntry;
  /** Videofila fra samme økt, når det finnes en. */
  video: RecordingEntry | null;
  /** Millisekundet raden dateres til, eller `null` når ingenting vet det. */
  atMs: number | null;
  /** Sekunder, eller `null` for ukjent. Se toppen av fila: 0 ER ukjent. */
  durationSec: number | null;
  /** Er det video i denne økta? Enten som sidefil, eller fordi raden selv er
   *  videofila (et opptak uten separat lydfil). */
  hasVideo: boolean;
  /** Notatet, hvis noen har skrevet ett. Vises som undertekst, ikke
   *  redigerbart — notatfeltet er ute av nivå 1 (eiervalg, canvas sett 3). */
  note: string | null;
  filename: string;
  /** Stien på disk, eller `null` når raden ikke har en. En slik rad kan ikke
   *  vises i Finder og kan ikke flyttes til papirkurven — bare ryddes bort. */
  path: string | null;
}

/** Er dette et tall vi kan bruke? */
function finite(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** Når økta begynte. Se toppen av fila. */
export function startedAtOf(entry: RecordingEntry): number | null {
  const started = finite(entry.startedAt);
  if (started !== null && started > 0) return started;
  const stamp = finite(entry.timestamp);
  return stamp !== null && stamp > 0 ? stamp : null;
}

/** Sekundene raden varte, eller `null`. 0 er ukjent — se toppen av fila. */
export function durationOf(entry: RecordingEntry): number | null {
  const seconds = finite(entry.durationSec);
  return seconds !== null && seconds > 0 ? seconds : null;
}

/**
 * Hva raden skal si om varigheten.
 *
 * ⚠️ Den tredje formen finnes fordi WKWebView-proben fant den på eierens egen
 * maskin: `spanOfSeconds` runder til nærmeste minutt, så et opptak på 20
 * sekunder ble «0 min» — den samme setningen P2 fjernet fra «Siste
 * opptak»-kortet, bare med motsatt årsak. Der var 0 UKJENT; her er den KJENT,
 * og likevel usann, for opptaket varte ikke null sekunder. Eierens profil har
 * fem slike testopptak fra Qu-5-runden.
 *
 * `under` er derfor «kjent, men kortere enn ett minutt», og grensen er nøyaktig
 * den `spanOfSeconds` runder på — ellers ville de to vært uenige om et opptak
 * på 45 sekunder.
 */
export type RowSpanKind = "unknown" | "under" | "span";

export interface RowSpan {
  kind: RowSpanKind;
  /** Sekundene, for `span`. 0 ellers. */
  seconds: number;
}

export function rowSpan(durationSec: number | null): RowSpan {
  const seconds = finite(durationSec);
  if (seconds === null || seconds <= 0) return { kind: "unknown", seconds: 0 };
  return Math.round(seconds / 60) === 0
    ? { kind: "under", seconds }
    : { kind: "span", seconds };
}

/**
 * Radmodellen: fold øktene, dater dem, og legg den nyeste øverst.
 *
 * Rekkefølgen er FØRST sortering og SÅ folding, fordi `pairRecordings` beholder
 * innkommende rekkefølge og forankrer paret der den første halvdelen står. Å
 * folde først og sortere etterpå ville betydd å sortere på en rad hvis dato
 * kunne komme fra hvilken som helst av de to filene.
 */
export function toLibraryRows(
  entries: readonly RecordingEntry[],
): LibraryRow[] {
  const sorted = sortNewestFirst(entries);
  return pairRecordings(sorted).map(({ r, videoEntry }, index) => {
    const path = (r.path ?? "").trim() || null;
    const note = (r.note ?? "").trim() || null;
    return {
      key: path ?? `row:${index}`,
      entry: r,
      video: videoEntry,
      atMs: startedAtOf(r),
      durationSec: durationOf(r),
      hasVideo: videoEntry !== null || isVideoPath(path),
      note,
      filename: r.filename ?? "",
      path,
    };
  });
}

/**
 * Nyeste først, stabilt.
 *
 * Ikke `sortRecordings` fra history-core: den sorterer på `timestamp`, og
 * radene dateres på `startedAt`. En liste som er sortert etter ett tall og
 * merket med et annet ser tilfeldig stokket ut nøyaktig den dagen et opptak
 * varte lenger enn de andre.
 *
 * Ukjent dato sorterer SIST: en rad vi ikke vet noe om skal ikke legge seg
 * øverst i lista over det man nettopp tok opp.
 */
export function sortNewestFirst(
  entries: readonly RecordingEntry[],
): RecordingEntry[] {
  return entries
    .map((entry, index) => ({ entry, index }))
    .sort((a, b) => {
      const at = startedAtOf(a.entry);
      const bt = startedAtOf(b.entry);
      if (at === null && bt === null) return a.index - b.index;
      if (at === null) return 1;
      if (bt === null) return -1;
      return bt - at || a.index - b.index;
    })
    .map((x) => x.entry);
}

/**
 * Videofil? Etternavnet avgjør, som i `isVideoRow`.
 *
 * Egen liten variant fordi `isVideoRow` også har `note === 'Video'` som
 * reservevei for rader importert fra Electron-historikken — den er riktig der,
 * og gal her: notatet vises nå som undertekst på raden, og en frivillig som
 * skriver «Video» i et notat skal ikke få en Video-brikke på en lydfil.
 */
const VIDEO_EXTS = new Set([
  "mp4",
  "mov",
  "mkv",
  "m4v",
  "webm",
  "avi",
  "wmv",
  "ts",
  "mts",
  "m2ts",
  "flv",
  "3gp",
  "asf",
  "f4v",
]);

export function isVideoPath(path: string | null): boolean {
  if (!path) return false;
  const match = /\.([^./\\]+)$/.exec(path);
  return match ? VIDEO_EXTS.has(match[1].toLowerCase()) : false;
}

// ── Søket ───────────────────────────────────────────────────────────────────

/**
 * Treffer raden søket?
 *
 * Ordrett legacys egen regel (`runSearch` i `pages/search-page.ts`): filnavn og
 * notat søkes ufølsomt for store bokstaver, DATOEN følsomt — den er en
 * ISO-streng, og «08» skal treffe måneden, ikke bli lowercase-et til noe annet.
 */
export function matchesQuery(entry: RecordingEntry, query: string): boolean {
  const raw = query.trim();
  const needle = raw.toLowerCase();
  return (
    (entry.filename ?? "").toLowerCase().includes(needle) ||
    (entry.date ?? "").includes(raw) ||
    (entry.note ?? "").toLowerCase().includes(needle)
  );
}

/** Radene søket slipper gjennom. En for kort spørring filtrerer ingenting. */
export function filterEntries(
  entries: readonly RecordingEntry[],
  query: string,
): RecordingEntry[] {
  const raw = query.trim();
  if (raw.length < MIN_QUERY_LENGTH) return entries.slice();
  return entries.filter((entry) => matchesQuery(entry, raw));
}

// ── Tellelinja ──────────────────────────────────────────────────────────────

/**
 * Sekundene radene til sammen holder.
 *
 * Summert over RADENE og ikke over oppføringene, som er forskjellen på en økt
 * og to filer: `historyTotals` i history-core summerer `durationSec` for hver
 * oppføring, så et opptak med kamera — som er to rader i basen — teller sin
 * egen lengde to ganger. Legacys statistikklinje gjør nettopp det.
 *
 * Rader uten kjent varighet legger til null, ikke 0 med en påstand rundt.
 */
export function totalSeconds(rows: readonly LibraryRow[]): number {
  return rows.reduce((sum, row) => sum + (row.durationSec ?? 0), 0);
}

// ── De to setningene om dager ───────────────────────────────────────────────
//
// Begge er tellende, og INGEN av dem får være en `tn()`-nøkkel:
// `check-i18n-plurals.mjs` krever hver flertallsgruppe i ALLE sju språk med
// riktige CLDR-kategorier og har ingen unntak for de fem som er pauset — en ny
// tellende nøkkel ville altså krevd polske flertallsformer midt i pausen som
// finnes for å slippe akkurat det. Så: kjernen velger FORMEN, og hver form har
// en `tf()`-nøkkel som er riktig for hele tallområdet den faktisk vises for.

/** Hva bunnlinjas venstre halvdel skal si om automatisk sletting. */
export type AutoDeleteKind = "off" | "oneDay" | "days";

export interface AutoDeleteLine {
  kind: AutoDeleteKind;
  days: number;
}

/** `autoDeleteDays` → formen. 0 (og alt ugyldig) er «av». */
export function autoDeleteLine(
  days: number | null | undefined,
): AutoDeleteLine {
  const value = finite(days);
  if (value === null || value < 1) return { kind: "off", days: 0 };
  const whole = Math.floor(value);
  return whole === 1
    ? { kind: "oneDay", days: 1 }
    : { kind: "days", days: whole };
}

/** Hva en papirkurv-rad skal si om tiden den har igjen. */
export type DueKind = "now" | "tomorrow" | "days";

export interface DueLine {
  kind: DueKind;
  days: number;
}

/**
 * `daysLeft` fra `toTrashRows` → formen.
 *
 * 0 betyr at neste opprydding tar den — sveipen går hver 12. time
 * (`src-tauri/src/trash/sweep.rs`), ikke ved midnatt, så «Slettes i dag» ville
 * vært en påstand om et tidspunkt ingen kjenner.
 */
export function dueLine(daysLeft: number): DueLine {
  const value = finite(daysLeft);
  if (value === null || value <= 0) return { kind: "now", days: 0 };
  const whole = Math.floor(value);
  return whole === 1
    ? { kind: "tomorrow", days: 1 }
    : { kind: "days", days: whole };
}
