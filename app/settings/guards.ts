/**
 * Vakten: en endring som kan koste deg opptaket spør først.
 *
 * Å bytte lydenhet fire minutter før gudstjenesten er endringen som stille
 * koster deg opptaket — så den får et spørsmål. Men BARE da. En vakt som
 * spør hver gang blir bakgrunnsstøy, og bakgrunnsstøy klikkes bort uten å
 * leses, som er verre enn ingen vakt.
 *
 * Terskelen er `guardReasonFor` i `@lib/ui/bind-setting-core` med
 * `WAKE_LEAD_MINUTES` (10) — den samme grensen backenden bruker til å arme
 * OS-vekkingen, altså vinduet der maskinen kanskje allerede holder på å våkne
 * for slotten. Ingen ny grense her: én terskel, ett sted.
 */

import {
  guardReasonFor,
  minutesUntil,
  type SettingValue,
} from "@lib/ui/bind-setting-core";
import { WAKE_LEAD_MINUTES } from "@lib/status/next-recording-core";

import { t, tf, tn } from "../i18n";
import { nextRecording } from "../state/next-recording";
import { confirmDialog } from "../ui/dialog";
import type { GuardDescriptor } from "./use-setting-core";

/**
 * `confirmIf` for `useSetting`. `what` navngir endringen i spørsmålet («Bytte
 * lydenhet»), så dialogen handler om det brukeren nettopp trykket på og ikke
 * om en generisk advarsel.
 *
 * Leser signalet med `peek()`: dette kalles fra en hendelse, ikke fra en
 * render, og et abonnement herfra ville knyttet en komponent til noe den ikke
 * viser.
 */
export function recordingImminentGuard(
  what: string,
): (value: SettingValue) => GuardDescriptor | null {
  return () => {
    const state = nextRecording.peek();
    const now = Date.now();
    const reason = guardReasonFor(
      { isRecording: state.isRecording, nextAtMs: state.next?.atMs ?? null },
      now,
      WAKE_LEAD_MINUTES,
    );
    if (!reason) return null;
    return {
      title: tf("guard.title", { what }),
      message:
        reason === "recording"
          ? t("guard.duringRecording")
          : tn(
              "guard.beforeRecording",
              minutesUntil(state.next?.atMs ?? now, now),
            ),
      confirmLabel: t("guard.confirm"),
      cancelLabel: t("guard.cancel"),
    };
  };
}

/**
 * Den samme vakten, imperativ — for flater som ikke er skjemakontroller.
 * Enhetsvelgeren er en liste med kort, ikke en `<select>`, og kan ikke gå
 * gjennom `useSetting`. Løses med om endringen skal fortsette.
 */
export async function confirmIfRecordingImminent(
  what: string,
): Promise<boolean> {
  const guard = recordingImminentGuard(what)(null);
  if (!guard) return true;
  return confirmDialog({ ...guard, danger: true });
}
