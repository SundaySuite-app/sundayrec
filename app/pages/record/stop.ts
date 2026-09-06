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
