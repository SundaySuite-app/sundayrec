/**
 * Retensjonspasset — løftet «Slettes automatisk etter {n} dager», holdt.
 *
 * ## Historien
 *
 * Bryteren «Slett gamle opptak» på Avansert og bibliotekfotens «Slettes
 * automatisk …» har stått der siden Electron-appen — og ingenting har noen
 * gang slettet (V1/PR3-funnet; hele historien står på `recordings_prune` i
 * `src-tauri/src/commands/db.rs`). Eierbeslutningen 2026-08-31: retensjon
 * FLYTTER til papirkurven, som begge UI-tekstene lover, og papirkurvens egen
 * 30-dagers tømming er den faktiske slettingen. Dette er oppkoblingen.
 *
 * ## Hvorfor passet bor her og ikke i en Rust-sweep
 *
 * Papirkurvens tømming er en Rust-tick (`trash::sweep`) fordi dens jobb er
 * stille disiplin: en full disk er feilen, og ingen trenger å få det med seg.
 * Retensjonen flytter OPPTAK AV GUDSTJENESTER, og eieren valgte at den skal si
 * fra når den gjør det — første pass etter denne oppdateringen kan flytte alt
 * som har hopet seg opp mens løftet ikke ble holdt. Da må passet leve der
 * toasten og butikkene bor, så meldingen og tallene på skjermen kommer fra
 * SAMME lesning som flyttingen. En Rust-tick med en event tilbake hadde vært
 * enda en skjøt av akkurat den typen `reference-seam-bugs` handler om.
 *
 * ## Tidene
 *
 * Ett pass ved oppstart, så hver 12. time — samme rytme som papirkurv-sweepen,
 * og av samme grunn: en maskin som står på i fjorten dager skal fortsatt holde
 * løftet sitt. Backend-kommandoen leser `autoDeleteDays` selv per pass, så en
 * bruker som slår retensjonen på midt i økta blir plukket opp av neste tick
 * uten at noen abonnerer på innstillingen her.
 */

import { t, tn } from "../i18n";
import { navigate } from "../router/router";
import { toast } from "../ui/toast";
import { loadRecordingCount } from "./recordings";
import { loadTrash } from "./trash";

/** Samme rytme som `trash::sweep` i Rust: oppstart + hver 12. time. */
const TICK_MS = 12 * 60 * 60 * 1000;

/**
 * Toastens tid. Lengre enn en vanlig kvittering: den kan stå der idet en
 * frivillig fremdeles henger av seg jakka, og den bærer en handling. `0`
 * (blir stående) er reservert feil — dette er en kvittering, ikke en alarm.
 */
export const TOAST_MS = 10_000;

/**
 * Kjør ett pass. Shimmens `recordingsPrune` løser alltid (en feilet IPC
 * svarer `disabled`), så passet kan ikke kaste — og et pass som ikke flyttet
 * noe er stille, akkurat som papirkurv-sweepen.
 */
export async function runRetentionPass(): Promise<void> {
  const summary = await window.api.recordingsPrune();
  if (summary.disabled || summary.moved <= 0) return;

  // Butikkene FØR toasten: når meldingen kan leses skal tallene bak den
  // (papirkurv-tellinga, biblioteklista) allerede stemme med den.
  await Promise.all([loadTrash(), loadRecordingCount()]);

  // `trash.*`, ikke `app.*`: nøklene bor hos papirkurv-søsknene sine, som er
  // oversatt i alle sju katalogene — en flertallsgruppe kan ikke bo i det
  // pausede `app.`-subtreet (`app/lib/i18n.test.ts` sveiper alle sju).
  toast("info", tn("trash.retentionMoved", summary.moved), {
    durationMs: TOAST_MS,
    action: {
      label: t("trash.retentionShowTrash"),
      onClick: () => navigate("edit", { tab: "trash" }),
    },
  });
}

let dispose: (() => void) | null = null;

/** Arm passet: ett nå, så hver 12. time. Idempotent, som de andre init-ene. */
export function initRetention(): () => void {
  if (dispose) return dispose;
  void runRetentionPass();
  const tick = setInterval(() => void runRetentionPass(), TICK_MS);
  dispose = () => {
    clearInterval(tick);
    dispose = null;
  };
  return dispose;
}
