/**
 * Lyd-steget har ÉN oppskrift (F3): «Podkastklar lyd» — `speech-clear` (−16 LUFS, sann topp
 * under taket) med Tales rensing foran — eller ingenting. Bryteren er hele valget.
 *
 * Før F3 het denne fila tabellen over tre profiler («Tale», «Tale og musikk», «Ingen») og
 * hvilket mastring-preset hver av dem sendte. De andre presettene, mikseren og
 * kanalreparasjonen er fjernet; det som er igjen her er de to rene tingene lyd-steget
 * trenger: hva eksporten og lyttingen sender, og utsnittet lyttingen spiller.
 *
 * Ren og node-testet med vilje: en tabell noen har lest gjennom, ikke betingelser spredt
 * utover en eksportfunksjon.
 */

/** Bryteren «Podkastklar lyd» skrur den på som standard. */
export const DEFAULT_PODCAST_AUDIO = true;

/** Feltene en eksport (og en lytting) arver fra lyd-steget. */
export interface SoundExportFields {
  /** Bakenden kjører `speech-clear`-oppskriften. Fraværende = filen slik den ble tatt opp. */
  podcastAudio?: true;
}

/**
 * Lyd-steget → lydfeltene i nyttelasten. To utfall: på gir `podcastAudio`, av gir ingenting
 * og fila kommer ut slik den gikk inn. Gamle verdier (presetnavn, mikser) finnes ikke lenger
 * og leses aldri.
 */
export function soundExportFields(podcastAudio: boolean): SoundExportFields {
  return podcastAudio ? { podcastAudio: true } : {};
}

/**
 * Utsnittet før/etter-lyttingen bruker: 20 sekunder fra MIDTEN av det som beholdes.
 *
 * Midten, og ikke starten: det første minuttet er ofte en innledning som er
 * tatt opp mens folk setter seg, og det er ikke materialet man vurderer en
 * lydprofil på. Midten av det som blir IGJEN etter kuttene er det nærmeste vi
 * kommer.
 *
 * Klemt slik at hele utsnittet ligger inne i fila — bakenden klemmer også
 * (`clamp_preview_start`), men et startpunkt som ligger 5 sekunder før slutten
 * gir 5 sekunders lytting og ingen beskjed om hvorfor.
 */
export const LISTEN_SPAN_SEC = 20;

export function listenStartSec(
  window: { start: number; end: number } | null,
  durationSec: number,
): number {
  if (!Number.isFinite(durationSec) || durationSec <= 0) return 0;
  const mid =
    window && window.end > window.start
      ? (window.start + window.end) / 2
      : durationSec / 2;
  const latest = Math.max(0, durationSec - LISTEN_SPAN_SEC);
  return Math.min(Math.max(0, mid - LISTEN_SPAN_SEC / 2), latest);
}

/**
 * B3b — tre ord til den frivillige: hva «Podkastklar lyd» gjorde med akkurat dette opptaket.
 *
 * Bakenden (`editor_tale_said`) svarer med stabile koder i kjedens rekkefølge,
 * aldri prosa. Her blir de til nøkler under `app.editor.said`:
 *
 *   `hum` `noise` `reverb` `clicks` → én linje hver («Vi fjernet brum fra strømnettet»)
 *   ingenting gjort                 → `clean` («Opptaket var rent.»)
 *   analysen feilet, eller bryteren er av → ingenting. Å si «rent» om en fil vi ikke fikk
 *   lest er en løgn.
 *
 * En ukjent kode (en ny trigger i en nyere bakende) hoppes over, så skjermen
 * aldri maler en råkode.
 */
const SAID_CODES = ["hum", "noise", "reverb", "clicks"] as const;

export function taleSaidKeys(
  podcastAudio: boolean,
  said: { analysed: boolean; said: readonly string[] } | null,
): string[] {
  if (!podcastAudio || !said?.analysed) return [];
  const known = SAID_CODES.filter((c) => said.said.includes(c));
  return known.length > 0 ? known : ["clean"];
}
