/**
 * Appversjonen, som et signal.
 *
 * Én linje nederst i skinnen, og likevel verdt en modul: den er det eneste
 * stedet en frivillig kan lese hvilken versjon maskinen faktisk kjører, og det
 * er det første spørsmålet i enhver feilsøking. `null` betyr «ikke lest ennå»
 * — ikke «ukjent versjon», og skinnen viser da ingenting i stedet for en
 * strek som ser ut som et svar.
 */

import { signal } from "@preact/signals";

export const appVersion = signal<string | null>(null);

export async function loadAppVersion(): Promise<void> {
  try {
    // Shimmen svarer «—» når `app_info` feiler; det er ikke en versjon.
    const version = await window.api.getAppVersion();
    appVersion.value = version && version !== "—" ? version : null;
  } catch (err) {
    console.warn("[app-info] kunne ikke lese versjonen:", err);
  }
}
