/**
 * BIBLIOTEK — jobb nr. 2 av fire: finn opptaket igjen.
 *
 * Historikk og Rediger-inngangen er ett sted nå (eiervalg, canvas sett 3). Det
 * som forsvant på veien er ikke gjemt, det er borte med vilje:
 *
 *   - **Sorterbare kolonner.** Fem overskrifter man kan klikke på er fem valg
 *     å ta stilling til for å finne det man tok opp sist. Nyeste først, alltid.
 *   - **Filteret «Alle · Lyd · Video».** Video er en BRIKKE på raden nå, så
 *     filteret ville skjult rader for å svare på et spørsmål brikka allerede
 *     svarer på.
 *   - **Notat-modalen.** Notatet VISES fortsatt (det er data eieren har
 *     skrevet, og det slettes ikke), men det redigeres ikke herfra. Atlaset
 *     talte 24 nøkler for en funksjon som i praksis brukes til å skrive
 *     «Gudstjeneste» ved siden av en dato som allerede sier det.
 *   - **«Slett alle», «Rydd opp», «Slett feil» og «⋯».** Tre destruktive
 *     masse-handlinger bak en meny, på en side en frivillig kommer til for å
 *     finne ETT opptak.
 *
 * ## Slett uten dialog, med Angre
 *
 * Et opptak som slettes flyttes til papirkurven — filen, sidevognene og
 * videosøsteren — og ligger der i 30 dager. En beslutning man kan ta tilbake
 * fortjener ikke en modal i veien; raden går, og toasten tilbyr «Angre», som
 * legger tilbake ALT ett klikk flyttet. Angre-vinduet er legacys eget, se
 * `UNDO_MS`.
 *
 * ⚠️ Den ene raden som IKKE er angrbar er den der fila allerede var borte fra
 * disken. Da er det ingenting å flytte, og alt som finnes er en historikkrad
 * som peker på ingenting — den ryddes bort (`recordings_delete`), akkurat som
 * legacy gjør det, og toasten kommer da uten «Angre» i stedet for å tilby en
 * knapp som ikke kunne gjort noe.
 *
 * ## «Rediger» er primærknappen (P4a)
 *
 * Canvasens 3.1 har «Rediger» som radens primærknapp, og fra P4a finnes flaten
 * den åpner: `app/editor/`, steg 1 «Klipp». «Vis i Finder» er sekundær nå — den
 * gjør fortsatt noe, den er bare ikke det man kom hit for.
 *
 * Raden sender med DATOEN sin. Editoren kan ikke lese den ut av fila, og den er
 * overskriften der akkurat som her.
 *
 * ## Filnavnet står under datoen
 *
 * Canvasen setter datoen som tittel og viser ikke filnavnet. Det gjør vi
 * likevel, dempet, under: søkefeltet heter «Søk etter dato eller navn», og et
 * søk som treffer noe skjermen aldri viser er en liten løgn fortalt ved hvert
 * besøk. Datoen er fortsatt det man leser først.
 */

import { useEffect, useState } from "preact/hooks";

import type { TrashEntry } from "@lib/pages/trash-core";

import { openInEditor } from "../../editor/entry";
import { locale, t, tf } from "../../i18n";
import { navigate } from "../../router/router";
import { loadRecordingCount, recordings } from "../../state/recordings";
import { settings } from "../../state/settings";
import { loadTrash, trashCount } from "../../state/trash";
import { Button } from "../../ui/Button/Button";
import { Chip } from "../../ui/Chip/Chip";
import { EmptyState } from "../../ui/EmptyState/EmptyState";
import { TextField } from "../../ui/TextField/TextField";
import { toast } from "../../ui/toast";
import { spanOfSeconds } from "../record/record-core";
import { spanText } from "../record/span-text";
import {
  autoDeleteLine,
  filterEntries,
  rowSpan,
  toLibraryRows,
  totalSeconds,
  type LibraryRow,
} from "./library-core";
import styles from "./library.module.css";

/** Skilletegn mellom fakta på én linje. Et tegn, ikke prosa. */
const DOT = " · ";

/** Varighet vi ikke kjenner. Ikke «0 min» — se `library-core`. */
const UNKNOWN = "—";

/**
 * Sekunder → setningen raden viser.
 *
 * Tre utfall, og det midterste er WKWebView-probens funn: et opptak på 20
 * sekunder runder til «0 min», som er kjent OG usant. Se `rowSpan`.
 */
function spanLabel(durationSec: number | null): string {
  const span = rowSpan(durationSec);
  if (span.kind === "unknown") return UNKNOWN;
  if (span.kind === "under") return t("app.library.underMinute");
  return spanText(spanOfSeconds(span.seconds));
}

/**
 * Hvor lenge «Angre» står.
 *
 * Legacys eget tall (`trashedToast` i `pages/history.ts`), og grunnen er
 * legacys egen: lenge nok til å ombestemme seg etter et bomklikk, kort nok til
 * ikke å bli stående over lista man jobber i. Husets standard for en `info` er
 * 3,2 sekunder, og det er for kort til å rekke å lese at det finnes en vei
 * tilbake.
 */
const UNDO_MS = 9000;

/**
 * Papirkurven som `route.tab`, ikke som en egen side.
 *
 * Samme valg som de fem oppsett-skjermene: `tab` er allerede navnerommet
 * ruteren oversetter alt det gamle inn i, og en egen rute-akse ville betydd to
 * tabeller å holde i takt for én informasjonsarkitektur. Skinnen står på
 * BIBLIOTEK hele veien — papirkurven er et sted inne i biblioteket, ikke et
 * fjerde sted i appen.
 */
export const TRASH_TAB = "trash";

/** Overskriften BIBLIOTEK skal ha for denne fanen. Utelatt = destinasjonens
 *  eget navn, «Bibliotek». */
export function libraryHeading(tab: string | undefined): string | undefined {
  return tab === TRASH_TAB ? t("trash.title") : undefined;
}

export function LibraryPage() {
  const entries = recordings.value;
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState<string | null>(null);

  // Leses når SIDEN åpnes: et opptak som ble tatt mens man sto på OPPTAK skal
  // være her når man går hit, og papirkurven kan ha blitt tømt av sveipen.
  useEffect(() => {
    void loadRecordingCount();
    void loadTrash();
  }, []);

  const rows =
    entries === null ? null : toLibraryRows(filterEntries(entries, query));
  const anyRecordings = entries !== null && entries.length > 0;

  async function remove(row: LibraryRow): Promise<void> {
    if (busy) return;
    setBusy(row.key);
    try {
      const moved = await trashRow(row);
      await Promise.all([loadRecordingCount(), loadTrash()]);
      toast("info", t("app.library.movedToTrash"), {
        durationMs: UNDO_MS,
        action: moved.length
          ? { label: t("trash.undo"), onClick: () => void undoTrash(moved) }
          : undefined,
      });
    } catch (err) {
      console.warn("[library] kunne ikke slette:", err);
      toast("error", t("history.deleteFailed"));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div class={styles.page}>
      <div class={styles.head}>
        {rows && rows.length > 0 ? (
          <p data-testid="library-sub" class={styles.sub}>
            {countLine(rows)}
          </p>
        ) : (
          <span />
        )}
        {/*
          Søkefeltet står bare når det finnes noe å søke i. Et felt over en tom
          liste er en kontroll som ikke kan gjøre noe, og den formen lærer folk
          at kontrollene her ikke henger sammen med det de ser.
        */}
        {anyRecordings ? (
          <div class={styles.search}>
            <span id="library-search-label" class={styles.srOnly}>
              {t("app.library.search")}
            </span>
            <TextField
              value={query}
              onInput={setQuery}
              placeholder={t("app.library.search")}
              labelId="library-search-label"
              testId="library-search"
            />
          </div>
        ) : null}
      </div>

      {/* Ikke lest ennå ⇒ ingen påstand i noen retning. */}
      {rows === null ? (
        <span />
      ) : !anyRecordings ? (
        <EmptyState
          testId="library-empty"
          title={t("app.library.empty")}
          description={t("app.library.emptyDesc")}
          action={
            <Button
              variant="primary"
              testId="library-go-record"
              onClick={() => navigate("record")}
            >
              {t("app.library.goRecord")}
            </Button>
          }
        />
      ) : rows.length === 0 ? (
        // En egen tilstand, ikke «Ingen opptak ennå»: arkivet er fullt, det er
        // spørringen som er tom. Atlaset fant tre distinkte tomtilstander her;
        // de to som betyr noe er beholdt.
        <EmptyState
          testId="library-no-hits"
          title={tf("app.library.noHits", { q: query.trim() })}
          description={t("app.library.noHitsDesc")}
        />
      ) : (
        <div class={styles.list}>
          {rows.map((row) => (
            <Row
              key={row.key}
              row={row}
              busy={busy === row.key}
              onDelete={() => void remove(row)}
            />
          ))}
        </div>
      )}

      <Foot />
    </div>
  );
}

/** «Opptak: 14 · 13 t 40 min». Beskriver radene som FAKTISK står under den —
 *  en tellelinje som beskriver arkivet mens tabellen viser et søk er to
 *  setninger om to forskjellige ting, ved siden av hverandre. */
function countLine(rows: readonly LibraryRow[]): string {
  const total = rowSpan(totalSeconds(rows));
  return [
    tf("app.library.count", { n: rows.length }),
    // `unknown` her betyr at INGEN av radene har en varighet — da er det
    // ingenting å si om summen, og «—» ville vært en gåte og ikke et svar.
    total.kind === "unknown" ? "" : spanLabel(total.seconds),
  ]
    .filter(Boolean)
    .join(DOT);
}

// ── Raden ───────────────────────────────────────────────────────────────────

function Row({
  row,
  busy,
  onDelete,
}: {
  row: LibraryRow;
  busy: boolean;
  onDelete: () => void;
}) {
  return (
    <div
      data-testid="library-row"
      data-path={row.path ?? undefined}
      class={styles.row}
    >
      <div class={styles.grow}>
        <div data-testid="library-row-when" class={styles.title}>
          {rowTitle(row)}
        </div>
        <div class={styles.meta}>
          <span data-testid="library-row-span">
            {spanLabel(row.durationSec)}
          </span>
          {row.hasVideo ? (
            <Chip tone="neutral" testId="library-row-video">
              {t("app.library.video")}
            </Chip>
          ) : null}
        </div>
        <div data-testid="library-row-name" class={styles.muted}>
          {row.filename}
        </div>
        {row.note ? (
          <div data-testid="library-row-note" class={styles.muted}>
            {row.note}
          </div>
        ) : null}
      </div>
      <div class={styles.acts}>
        {/*
          «Rediger» er radens PRIMÆRKNAPP (canvas 3.1). P3 satte «Vis i Finder»
          her fordi redigeringsflaten ikke fantes ennå og en knapp til en side
          som ikke finnes lærer en frivillig at knappene ikke er til å stole
          på. Nå finnes den, så knappen er byttet og «Vis i Finder» er
          sekundær — den gjør fortsatt noe, den er bare ikke det man kom for.
        */}
        <Button
          variant="primary"
          disabled={row.path === null}
          disabledReason={t("app.done.revealFailed")}
          testId="library-row-edit"
          onClick={() => openInEditor(row.path as string, row.atMs)}
        >
          {t("nav.editor")}
        </Button>
        {/*
          Én «Vis i Finder» også for en økt med kamera: de to filene ligger side
          om side i samme mappe, så Finder viser begge. Legacy hadde en egen
          knapp for videofila, som er et andre klikk for å åpne det samme
          vinduet.
        */}
        <Button
          variant="secondary"
          disabled={row.path === null}
          disabledReason={t("app.done.revealFailed")}
          testId="library-row-reveal"
          onClick={() => void reveal(row.path)}
        >
          {t("app.done.show")}
        </Button>
        <Button
          variant="ghost"
          busy={busy}
          testId="library-row-delete"
          onClick={onDelete}
        >
          {t("app.library.delete")}
        </Button>
      </div>
    </div>
  );
}

/**
 * «Søndag 16. august 2026 · 11:00».
 *
 * Året er med, i motsetning til `intlParts`' `dateLong`: et bibliotek spenner
 * over år, og «søndag 16. august» er to forskjellige gudstjenester i en
 * menighet som har brukt appen i to sesonger.
 *
 * Ingen dato i det hele tatt ⇒ filnavnet er tittelen. En rad som bare sier «—»
 * er en rad man ikke kan kjenne igjen.
 */
function rowTitle(row: LibraryRow): string {
  if (row.atMs === null) return row.filename;
  const loc = locale.value;
  const when = new Date(row.atMs);
  const date = when.toLocaleDateString(loc, {
    weekday: "long",
    day: "numeric",
    month: "long",
    year: "numeric",
  });
  const time = when.toLocaleTimeString(loc, {
    hour: "2-digit",
    minute: "2-digit",
  });
  return `${capitalize(date, loc)}${DOT}${time}`;
}

/** Stor forbokstav på første tegn — `Intl` gir «søndag», og det er riktig midt
 *  i en setning og feil som tittel. Samme funksjon og samme grunn som
 *  `capitalizeFirst` i `record-core`, som ikke kan importeres hit uten å ta
 *  med en `Settings`-avhengighet raden ikke har bruk for. */
function capitalize(text: string, loc: string): string {
  return text ? text[0].toLocaleUpperCase(loc) + text.slice(1) : text;
}

// ── Bunnlinja ───────────────────────────────────────────────────────────────

/**
 * To setninger, og den ene av dem er hele funn 9 i atlaset.
 *
 * «Papirkurv» står ALLTID. Legacy skjuler lenken når kurven er tom, og lukker
 * samtidig visningen hvis den står åpen — så en frivillig som slettet noe i går
 * og leter etter det i dag finner ingen dør hvis sveipen har vært innom i
 * mellomtiden. Den scenen er ikke engang fotograferbar i atlaset.
 */
function Foot() {
  const days = autoDeleteLine(settings.value.autoDeleteDays);
  const inTrash = trashCount.value;

  return (
    <div class={styles.foot}>
      <span class={styles.footNote}>
        <span data-testid="library-autodelete">
          {days.kind === "off"
            ? t("app.library.autoDeleteOff")
            : days.kind === "oneDay"
              ? t("app.library.autoDeleteDay")
              : tf("app.library.autoDeleteDays", { n: days.days })}
        </span>
        <Button
          variant="ghost"
          testId="library-autodelete-change"
          onClick={() =>
            navigate("setup", { tab: "advanced", anchor: "autodelete" })
          }
        >
          {t("app.setup.change")}
        </Button>
      </span>
      <Button
        variant="ghost"
        testId="library-trash-open"
        onClick={() => navigate("library", { tab: "trash" })}
      >
        {inTrash === null
          ? // Ikke lest ennå: si «Papirkurv», ikke «(0)». Et tall vi ikke har
            // er ikke null.
            t("trash.title")
          : inTrash === 0
            ? t("trash.alreadyEmpty")
            : tf("app.library.trashCount", { n: inTrash })}
      </Button>
    </div>
  );
}

// ── Sømmen mot papirkurven ──────────────────────────────────────────────────

/**
 * Flytt én rads filer til papirkurven, og rydd bort det som ikke fantes.
 *
 * Ordrett legacys `trashRows`, og av legacys grunn: `trash_move` hopper over
 * det som ikke er på disken, så en historikkrad hvis fil noen slettet for hånd
 * ville blitt stående for alltid uten en vei ut. Den raden har ingenting å
 * gjenopprette, så den fjernes i stedet — og fordi den ikke er med i det som
 * ble flyttet, får toasten ingen «Angre» å tilby.
 */
async function trashRow(row: LibraryRow): Promise<TrashEntry[]> {
  const entries = row.video ? [row.entry, row.video] : [row.entry];
  const paths = entries
    .map((e) => e.path)
    .filter((p): p is string => typeof p === "string" && p.length > 0);
  const moved = paths.length ? await window.api.trashMove(paths) : [];
  const movedPaths = new Set(moved.map((e) => e.originalPath));
  for (const entry of entries) {
    if (entry.path && movedPaths.has(entry.path)) continue;
    if (entry.timestamp) await window.api.deleteHistoryEntry(entry.timestamp);
  }
  return moved;
}

/** Legg tilbake alt ETT slett flyttet. Delvis suksess sier fra — «Angre» som
 *  stille bare hentet halvparten er verre enn en beskjed om å se etter. */
async function undoTrash(entries: readonly TrashEntry[]): Promise<void> {
  let failed = 0;
  for (const entry of entries) {
    try {
      await window.api.trashRestore(entry.id);
    } catch (err) {
      failed += 1;
      console.warn("[library] kunne ikke legge tilbake:", err);
    }
  }
  await Promise.all([loadRecordingCount(), loadTrash()]);
  if (failed > 0) toast("warn", t("trash.undoFailed"));
}

/** «Vis i Finder». Sier fra når det ikke gikk — en knapp som stille ikke gjør
 *  noe er verre enn ingen knapp. */
async function reveal(path: string | null): Promise<void> {
  if (!path) return;
  const ok = await window.api.revealFile(path);
  if (!ok) toast("error", t("app.done.revealFailed"));
}
