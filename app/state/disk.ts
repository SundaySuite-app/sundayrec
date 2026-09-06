/**
 * Hvor mye plass er det igjen — i MINUTTER opptak, ikke i gigabyte.
 *
 * «412 GB ledig» svarer ikke på spørsmålet. «Plass til 1 time og 40 minutter»
 * gjør det, og det er den formen statuslinjens `lowdisk` trenger for i det
 * hele tatt å kunne stilles opp mot «neste opptak er 90 minutter».
 *
 * ⚠️ Regnestykket er en KOPI av `loadDiskSpace()` i
 * `legacy/renderer/pages/home.ts` (kbps per format, `bytes / (kbps · 125)`),
 * fordi originalen er en privat funksjon inne i en 1500-linjers DOM-modul som
 * `app/` ikke kan importere uten å dra hele legacy-treet med seg. Kopien er
 * bevisst og midlertidig: når hjemmesiden er portet i fase P finnes tallet ett
 * sted igjen. Den er ren og testet her, slik at de to i det minste kan
 * sammenlignes.
 */

import { signal } from "@preact/signals";

import { settings, type Settings } from "./settings";

/** Ledige byte på lagringsdisken, eller `null` før første lesning. */
export const diskFreeBytes = signal<number | null>(null);

/**
 * Samplingsraten vi ESTIMERER med. `auto` tar opp i enhetens egen rate, som
 * ingen kan vite uten å åpne enheten — 48 kHz er anslaget legacy også bruker.
 */
export function estimatedSampleRateHz(mode: string | null | undefined): number {
  if (mode === "r44100") return 44100;
  if (mode === "r96000") return 96000;
  return 48000;
}

/** Kilobit per sekund opptaket kommer til å bruke, gitt innstillingene. */
export function kbpsFor(s: Settings): number {
  const format = (s.format ?? "mp3").toLowerCase();
  const stereo = s.channels === "stereo";
  if (format === "wav") {
    return Math.round(
      (estimatedSampleRateHz(s.sampleRateMode) * (stereo ? 2 : 1) * 16) / 1000,
    );
  }
  // FLAC komprimerer, men hvor mye avhenger av materialet. Tallene er
  // legacy-anslaget: rundt halvparten av WAV.
  if (format === "flac") return stereo ? 600 : 350;
  const bitrate = parseInt(String(s.bitrate ?? 256), 10);
  return Number.isFinite(bitrate) && bitrate > 0 ? bitrate : 256;
}

/** Minutter opptak `freeBytes` holder til, eller `null` når vi ikke vet. */
export function roomMinutes(
  freeBytes: number | null,
  kbps: number,
): number | null {
  if (freeBytes === null || !Number.isFinite(freeBytes) || freeBytes < 0)
    return null;
  if (!Number.isFinite(kbps) || kbps <= 0) return null;
  // kbps · 125 = byte per sekund (1000 bit / 8).
  return Math.floor(freeBytes / (kbps * 125) / 60);
}

/** Minutter det er plass til nå, gitt innstillingene som gjelder. */
export function currentRoomMinutes(): number | null {
  return roomMinutes(diskFreeBytes.value, kbpsFor(settings.value));
}

/**
 * Les ledig plass. En feilet lesning lar den forrige verdien stå: at en IPC
 * bommet er ikke bevis for at disken er full — eller for at den er tom.
 */
export async function refreshDiskSpace(): Promise<void> {
  try {
    const disk = await window.api.getDiskSpace();
    const free = disk?.freeBytes;
    diskFreeBytes.value =
      typeof free === "number" && Number.isFinite(free) ? free : null;
  } catch (err) {
    console.warn("[disk] kunne ikke lese ledig plass:", err);
  }
}

/** Hvor ofte disken leses på nytt. Et opptak spiser plass i minutter, ikke
 *  sekunder — hyppigere ville vært en IPC per frame uten ny informasjon. */
const POLL_MS = 60_000;

let dispose: (() => void) | null = null;

export function initDisk(): () => void {
  if (dispose) return dispose;
  void refreshDiskSpace();
  const poll = setInterval(() => void refreshDiskSpace(), POLL_MS);
  dispose = () => {
    clearInterval(poll);
    dispose = null;
  };
  return dispose;
}
