/**
 * Å be om å få stoppe — og bare gjøre det hvis svaret er nei.
 *
 * ## Bekreftelsen er snudd med vilje
 *
 * Eiervalget (canvas sett 2, punkt 3): primærknappen er «Fortsett å ta opp».
 * Et uhell midt i prekenen skal koste ett klikk til, ikke opptaket.
 * `buildConfirm` gir BEKREFT-knappen primærplassen og Enter-tasten når
 * dialogen ikke er `danger` — så «fortsett» ER bekreftelsen her, og «stopp»
 * går den veien som ellers heter avbryt.
 *
 * Alternativet var `danger: true`, som gir avbryt Enter-plassen — men det maler
 * også stopp-knappen RØD, og rødt betyr én ting i denne appen: at det tas opp.
 * En rød stoppknapp midt i et rødt overlegg er nøyaktig den fargekollisjonen
 * sett 0 låste bort.
 *
 * ## F2-T3: Escape skal ikke kunne stoppe opptaket
 *
 * `cancelLabel` bærer «stopp» — den AKTIVE handlingen, ikke den vanlige
 * no-op-en avbryt-knappen ellers er. `DialogHost` lukker enhver dialog med
 * Escape (og et klikk på sløret) via KNAPPEN MED `isCancel`, uansett hva den
 * knappen betyr — og uten `escapeConfirms: true` er det nettopp «stopp»-
 * knappen. Bevist i praksis (`e2e/record.spec.ts`): et Escape-trykk kalte
 * `stop_recording` og satte opptaket i «Fullfører …» — nøyaktig den ene
 * uhellet bekreftelsen finnes for å forhindre, bare via tastaturet i stedet
 * for et feilklikk. `escapeConfirms: true` flytter Escape til «Fortsett å ta
 * opp» uten å røre stilen (fortsatt ghost, fortsatt ikke Enter-default) — se
 * `@lib/ui/dialog-core.ts`.
 *
 * ## Egen fil
 *
 * Både overleggets stoppknapp og menylinjens «Stopp opptak» skal gjennom det
 * SAMME spørsmålet. En av dem som spurte og en som ikke gjorde det er den
 * formen for uenighet som koster et opptak.
 *
 * ## `protectRecording` leses ikke
 *
 * Innstillingen har null Rust-lesere (ATLAS §2.6), det nye Avansert viser den
 * ikke, og bekreftelsen er en designbeslutning i sett 2 — ikke noe man skrur
 * av. Legacy-skallet har fortsatt bryteren sin.
 */

import { t, tf } from "../../i18n";
import {
  endSessionLocally,
  enterFinalizing,
  finalizing,
  sessionStartedAtMs,
} from "../../state/recording";
import { confirmDialog } from "../../ui/dialog";
import { formatClock } from "./record-core";

export async function confirmAndStop(): Promise<void> {
  // Et andre trykk mens motoren skriver ferdig er ikke en ny stoppforespørsel.
  if (finalizing.peek()) return;
  const startedAt = sessionStartedAtMs.peek();
  const keepRecording = await confirmDialog({
    title: t("app.overlay.stopQuestion"),
    message: tf("app.overlay.stopQuestionDesc", {
      elapsed: formatClock(startedAt === null ? 0 : Date.now() - startedAt),
    }),
    confirmLabel: t("app.overlay.keep"),
    cancelLabel: t("app.overlay.stopYes"),
    escapeConfirms: true,
  });
  if (keepRecording) return;

  enterFinalizing();
  try {
    await window.api.stopRecordingNow();
  } catch (err) {
    // Selve forespørselen feilet, så det kommer ingen terminal hendelse —
    // rydd her i stedet for å vente ut de 30 sekundene.
    console.error("[record] stop_recording feilet:", err);
    endSessionLocally();
  }
}
