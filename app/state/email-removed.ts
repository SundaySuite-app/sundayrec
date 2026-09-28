/**
 * «E-postvarsler er fjernet» — engangsbeskjeden.
 *
 * SundayRec kunne sende e-post når et opptak feilet. Det er fjernet: appen
 * sier fra med et systemvarsel på maskinen, og ingen innstilling kan slå det
 * av. En frivillig som HADDE e-postvarsel slått på fortjener å høre det fra
 * appen, ikke oppdage det den søndagen mailen uteble — så bakenden legger en
 * beskjed klar ved oppstart (`settings::email_cleanup`), og OPPTAK viser den
 * som et banner til den er lest.
 *
 * Lest én gang per oppstart, som resten av det OPPTAK henter: en beskjed som
 * skal vises én gang trenger ingen strøm av oppdateringer.
 */

import { signal } from "@preact/signals";

/** `true` mens banneret skal stå. */
export const emailRemovedNotice = signal(false);

let loaded = false;

/** Test-krok: glem at beskjeden er lest. */
export function resetEmailRemovedNoticeForTests(): void {
  loaded = false;
  emailRemovedNotice.value = false;
}

/** Spør bakenden én gang per oppstart. */
export async function loadEmailRemovedNotice(): Promise<void> {
  if (loaded) return;
  loaded = true;
  emailRemovedNotice.value = await window.api.noticeEmailRemovedPending();
}

/** «OK» — banneret går bort nå, og bakenden glemmer det for godt. */
export async function dismissEmailRemovedNotice(): Promise<void> {
  emailRemovedNotice.value = false;
  await window.api.noticeEmailRemovedDismiss();
}
