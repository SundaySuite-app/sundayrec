/**
 * Viser operativsystemet SundayRecs varsler?
 *
 * Hver feil appen melder, meldes som et systemvarsel på maskinen. Står varsler
 * av for SundayRec i macOS/Windows, får ingen beskjed — så kortet «Hvem får
 * beskjed?» følger svaret herfra (`decideNotify`).
 *
 * `null` = ikke spurt ennå. `"unknown"` = plattformen lar oss ikke vite det
 * (macOS i dag, se `src-tauri/src/notify/permission.rs`) — det er et svar, ikke
 * et «ikke ennå».
 *
 * Leses på nytt når vinduet får fokus igjen: den vanligste veien tilbake fra
 * «Åpne innstillinger» er at brukeren slår på varsler og klikker seg tilbake.
 */

import { signal } from "@preact/signals";
import type { NotificationPermission } from "@legacy/bindings/NotificationPermission";

export const notificationPermission = signal<NotificationPermission | null>(
  null,
);

let listening = false;

/** Spør bakenden, og lytt etter at vinduet får fokus igjen. */
export async function refreshNotificationPermission(): Promise<void> {
  if (!listening && typeof window.addEventListener === "function") {
    listening = true;
    window.addEventListener("focus", () => {
      void refreshNotificationPermission();
    });
  }
  notificationPermission.value = await window.api.notificationPermission();
}

/** The slice of `window.api` the test-notification flow needs. */
export interface TestNotificationApi {
  notificationRequestPermission: () => Promise<NotificationPermission>;
  notificationSendTest: () => Promise<boolean>;
}

/**
 * «Send testvarsel»: first let the backend ask macOS for permission (it does so
 * only when the status was never decided, and only inside the `.app`), show the
 * status it ends with on the card, THEN send the notification — so the very
 * first test on a fresh Mac is preceded by the system dialog, not silently lost.
 */
export async function sendTestNotification(
  api: TestNotificationApi,
): Promise<{ sent: boolean; permission: NotificationPermission }> {
  const permission = await api.notificationRequestPermission();
  notificationPermission.value = permission;
  const sent = await api.notificationSendTest();
  return { sent, permission };
}
