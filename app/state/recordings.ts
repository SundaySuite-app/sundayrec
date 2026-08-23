/**
 * Opptakene som ligger på maskinen.
 *
 * S1b holdt bare TALLET, fordi det var alt Bibliotek kunne si sant om ennå.
 * P2 trenger den nyeste raden i tillegg — «Siste opptak»-kortet på OPPTAK og
 * kvitteringens varighet og størrelse — så butikken holder LISTA, og tallet er
 * avledet av den. Én lesning, én sannhet: et tall og en liste som telles hver
 * for sig er to steder som kan bli uenige om hva som finnes.
 *
 * `null` betyr «ikke lest ennå» og er IKKE det samme som `0`. En feilet
 * lesning lar den forrige verdien stå — en IPC som bommet er ikke bevis for at
 * biblioteket er tomt, og «Ingen opptak ennå» på en maskin med tolv opptak er
 * nøyaktig den slags løgn hele redesignet handler om å slutte med.
 *
 * ⚠️ `recordings_list` bærer INGEN redigert/eksportert-status: raden er
 * `id, file_path, device_name, started_at, duration_ms, byte_size, created_at,
 * note` og ikke noe mer (`legacy/bindings/RecordingRow.ts`). Canvasens brikker
 * «Redigert» og «Eksportert» har derfor ingen kilde, og de er ikke med. Et
 * merke som gjettes er verre enn ingen merke.
 */

import { computed, signal } from "@preact/signals";
import type { RecordingEntry } from "@lib/../types";

/**
 * Radene, nyeste først — `recordings_list` er `ORDER BY created_at DESC`, og
 * api-shimmen filtrerer bort det som ligger i papirkurven.
 */
export const recordings = signal<RecordingEntry[] | null>(null);

/** Hvor mange det er. Avledet, så den aldri kan bli uenig med lista. */
export const recordingCount = computed(() => recordings.value?.length ?? null);

/** Det nyeste opptaket, eller `null` når det ikke finnes noe (eller vi ikke vet). */
export const lastRecording = computed(() => recordings.value?.[0] ?? null);

export async function loadRecordingCount(): Promise<void> {
  try {
    const rows = await window.api.getHistory();
    if (Array.isArray(rows)) recordings.value = rows as RecordingEntry[];
  } catch (err) {
    console.warn("[recordings] kunne ikke lese opptakene:", err);
  }
}
