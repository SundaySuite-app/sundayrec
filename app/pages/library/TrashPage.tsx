/**
 * PAPIRKURVEN — de 30 dagene et slettet opptak fortsatt finnes.
 *
 * Den ene skjermen i hele designet der noe skjer for godt, og derfor den ene
 * som har farlige dialoger. Alt annet i Bibliotek er angrbart, og gjøres uten
 * å spørre.
 *
 * ## De 30 dagene er bakendens tall, ikke skjermens
 *
 * `TRASH_KEEP_DAYS` i `@lib/pages/trash-core` speiler `AUTO_PURGE_DAYS` i
 * `src-tauri/src/trash/mod.rs`, og sveipen som håndhever den er ekte:
 * `trash::sweep::spawn` armes fra `setup`, første gang etter 90 sekunder og så
 * hver 12. time, og sletter det som er eldre enn 30 dager sammen med
 * historikkradene deres. Setningen på skjermen er altså ikke et løfte skjermen
 * gir — den beskriver noe som allerede kjører.
 *
 * ## ⚠️ Hva en papirkurv-rad IKKE kan si
 *
 * Canvasens 3.3 gir hver rad en dato og en varighet («Søndag 26. juli 2026 ·
 * 11:00 · 1 t 03 min»). Det finnes ingen kilde til dem her: `trash_list` svarer
 * med `TrashEntry`, som er `id, originalPath, trashedPath, name, deletedAt,
 * related, byteSize` og ikke noe mer. Historikkraden ligger riktignok igjen i
 * basen med både starttid og varighet — det er nettopp det som gjør at en
 * gjenoppretting gir tilbake notatet — men api-shimmens `getHistory` FILTRERER
 * bort alt som ligger i kurven, på originalstien, og det er det filteret som
 * hindrer at en slettet fil dukker opp som et opptak som finnes. Å be om de
 * radene ville krevd en ny vei rundt filteret.
 *
 * Så raden sier det den faktisk vet: filnavnet, når den ble slettet, og hvor
 * lenge det er igjen. En dato som gjettes er verre enn ingen dato.
 */

import { useEffect, useState } from "preact/hooks";

import {
  ageText,
  toTrashRows,
  TRASH_KEEP_DAYS,
  type TrashRow,
} from "@lib/pages/trash-core";

import { locale, t, tf, tn } from "../../i18n";
import { navigate } from "../../router/router";
import { loadRecordingCount } from "../../state/recordings";
import { loadTrash, trashEntries } from "../../state/trash";
import { Button } from "../../ui/Button/Button";
import { EmptyState } from "../../ui/EmptyState/EmptyState";
import { confirmDialog } from "../../ui/dialog";
import { toast } from "../../ui/toast";
import { formatBytes } from "../record/record-core";
import { dueLine } from "./library-core";
import styles from "./library.module.css";

/** Skilletegn mellom fakta på én linje. Et tegn, ikke prosa. */
const DOT = " · ";

export function TrashPage() {
  const entries = trashEntries.value;
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    void loadTrash();
  }, []);

  const rows =
    entries === null ? null : toTrashRows(entries.slice(), Date.now());

  /** Etter enhver endring: begge listene, fordi en gjenoppretting flytter en
   *  rad fra den ene til den andre og en tømming fjerner historikkraden. */
  async function refresh(): Promise<void> {
    await Promise.all([loadTrash(), loadRecordingCount()]);
  }

  async function restore(row: TrashRow): Promise<void> {
    if (busy) return;
    setBusy(row.id);
    try {
      await window.api.trashRestore(row.id);
      await refresh();
    } catch (err) {
      console.warn("[trash] kunne ikke legge tilbake:", err);
      toast("error", t("trash.restoreFailed"));
    } finally {
      setBusy(null);
    }
  }

  async function purgeOne(row: TrashRow): Promise<void> {
    if (busy) return;
    // Den ene handlingen i Bibliotek som ikke kan angres, og derfor den ene som
    // spør. `danger` gir AVBRYT Enter-plassen og maler «Slett for godt» rød og
    // SEKUNDÆR — aldri en rød primærknapp (canvas sett 7).
    const ok = await confirmDialog({
      title: tf("trash.confirmPurgeOne", { name: row.name }),
      message: t("trash.confirmEmptyBody"),
      confirmLabel: t("trash.deleteForever"),
      danger: true,
    });
    if (!ok) return;
    setBusy(row.id);
    try {
      await window.api.trashPurge([row.id]);
      await refresh();
    } catch (err) {
      console.warn("[trash] kunne ikke slette for godt:", err);
      toast("error", t("history.deleteFailed"));
    } finally {
      setBusy(null);
    }
  }

  async function purgeAll(count: number): Promise<void> {
    if (busy) return;
    const ok = await confirmDialog({
      // Antallet står i tittelen: forskjellen på å slette 2 og 200 opptak er
      // hele beslutningen. Katalognøkkelen er legacys egen og finnes i alle sju
      // språk — en ny tellende nøkkel ville krevd polske flertallsformer midt i
      // pausen som finnes for å slippe akkurat det.
      title: tn("trash.confirmEmpty", count),
      message: t("trash.confirmEmptyBody"),
      confirmLabel: t("trash.deleteForever"),
      danger: true,
    });
    if (!ok) return;
    setBusy("all");
    try {
      // Tom liste = tøm alt (`trash_purge` i Rust).
      await window.api.trashPurge([]);
      await refresh();
    } catch (err) {
      console.warn("[trash] kunne ikke tømme papirkurven:", err);
      toast("error", t("history.deleteFailed"));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div class={styles.page}>
      <div class={styles.trashHead}>
        <p data-testid="trash-lede" class={styles.sub}>
          {tf("app.library.trashLede", { n: TRASH_KEEP_DAYS })}
        </p>
        <Button
          variant="ghost"
          testId="trash-back"
          onClick={() => navigate("edit")}
        >
          {t("app.library.back")}
        </Button>
      </div>

      {rows === null ? (
        <span />
      ) : rows.length === 0 ? (
        // Tomtilstanden atlaset sier ikke finnes i dag — fordi inngangen til
        // den skjuler seg når kurven er tom (§5, funn 9). Ingen knapp: en tom
        // papirkurv er ikke et problem som skal løses.
        <EmptyState
          testId="trash-empty"
          title={t("trash.alreadyEmpty")}
          description={t("app.library.trashEmptyDesc")}
        />
      ) : (
        <div class={styles.list}>
          {rows.map((row) => (
            <Row
              key={row.id}
              row={row}
              busy={busy === row.id}
              onRestore={() => void restore(row)}
              onPurge={() => void purgeOne(row)}
            />
          ))}
        </div>
      )}

      {rows && rows.length > 0 ? (
        <div class={styles.trashFoot}>
          <Button
            variant="danger"
            busy={busy === "all"}
            testId="trash-empty-all"
            onClick={() => void purgeAll(rows.length)}
          >
            {t("trash.emptyAll")}
          </Button>
        </div>
      ) : (
        <span />
      )}
    </div>
  );
}

function Row({
  row,
  busy,
  onRestore,
  onPurge,
}: {
  row: TrashRow;
  busy: boolean;
  onRestore: () => void;
  onPurge: () => void;
}) {
  const due = dueLine(row.daysLeft);
  const size = formatBytes(row.byteSize, locale.value);
  const meta = [
    `${t("trash.deletedAt")} ${ageText(row.ageDays, ageWords())}`,
    size,
    // Si at følgesvennene ble med: en gjenoppretting som stille også henter ni
    // JSON-filer er greit, men det skal ikke være en overraskelse.
    row.relatedCount > 0 ? tn("trash.related", row.relatedCount) : "",
  ]
    .filter(Boolean)
    .join(DOT);

  return (
    <div data-testid="trash-row" class={styles.row}>
      <div class={styles.grow}>
        <div data-testid="trash-row-name" class={styles.title}>
          {row.name}
        </div>
        <div class={styles.meta}>
          <span data-testid="trash-row-due">
            {due.kind === "now"
              ? t("app.library.deleteSoon")
              : due.kind === "tomorrow"
                ? t("app.library.deleteTomorrow")
                : tf("app.library.deleteInDays", { n: due.days })}
          </span>
        </div>
        <div class={styles.muted}>{meta}</div>
      </div>
      <div class={styles.acts}>
        <Button
          variant="secondary"
          busy={busy}
          testId="trash-row-restore"
          onClick={onRestore}
        >
          {t("app.library.restore")}
        </Button>
        <Button
          variant="danger"
          busy={busy}
          testId="trash-row-purge"
          onClick={onPurge}
        >
          {t("app.library.deleteNow")}
        </Button>
      </div>
    </div>
  );
}

/** «i dag» / «i går» / «3 dager siden». `trash-core` tar ordene utenfra så den
 *  slipper i18n-importen; alle tre finnes i alle sju språk. */
function ageWords(): {
  today: string;
  yesterday: string;
  daysAgo: (n: number) => string;
} {
  return {
    today: t("trash.today"),
    yesterday: t("trash.yesterday"),
    daysAgo: (n: number) => tn("trash.daysAgo", n),
  };
}
