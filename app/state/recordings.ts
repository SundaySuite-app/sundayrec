/**
 * Hvor mange opptak som ligger på maskinen.
 *
 * Bare tallet, ikke listen: fase P bygger biblioteksiden, og det som trengs
 * FØR den er svaret på ett spørsmål — «er det noe her i det hele tatt?».
 * Uten det ville tomtilstanden på Bibliotek vært en påstand ingen har sjekket,
 * og «Ingen opptak ennå» på en maskin med tolv opptak er nøyaktig den slags
 * løgn hele redesignet handler om å slutte med.
 *
 * `null` betyr «ikke lest ennå» og er IKKE det samme som `0`. En feilet
 * lesning lar den forrige verdien stå — en IPC som bommet er ikke bevis for at
 * biblioteket er tomt.
 */

import { signal } from "@preact/signals";

export const recordingCount = signal<number | null>(null);

export async function loadRecordingCount(): Promise<void> {
  try {
    const rows = await window.api.getHistory();
    recordingCount.value = Array.isArray(rows) ? rows.length : null;
  } catch (err) {
    console.warn("[recordings] kunne ikke telle opptakene:", err);
  }
}
