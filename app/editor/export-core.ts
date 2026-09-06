/**
 * Eksportens avgjørelser — filnavnet, størrelsen og feilen, som ren aritmetikk.
 *
 * Atlasets §3d teller ti rader i eksportmodalen, med bitrate, bitdybde,
 * videokodek og «Bithybde» (skrivefeilen som er sendt ut i alle sju språk).
 * Canvasens 4.3 er to spørsmål: hvilket format, og hvor. Alt annet følger av
 * kvalitetsvalget i Oppsett eller av fila selv, og det som følger av noe skal
 * regnes ut ett sted og testes der.
 */

import { errorCode } from "@lib/error-code-core";

/** De tre formatene canvasen tilbyr. Bakenden kjenner flere (aac, m4a, caf …),
 *  men et valg med sju alternativer er ikke et valg — det er en meny. */
export type ExportFormat = "mp3" | "flac" | "wav";

export const EXPORT_FORMATS: readonly ExportFormat[] = ["mp3", "flac", "wav"];

/** Formatet en frivillig får uten å velge. Minst fil, spilles overalt. */
export const DEFAULT_EXPORT_FORMAT: ExportFormat = "mp3";

/** Containeren en video-eksport havner i. Ett format, ikke tre — MOV og MKV
 *  er valg ingen frivillig har en mening om, og mp4 spilles overalt. */
export const VIDEO_FORMAT = "mp4";
/** Kodeken video-eksporten bruker. H.264 er den universelle. */
export const VIDEO_CODEC = "h264";

/** Bitraten mp3 får når `settings.bitrate` er tom eller tull. Legacys eget
 *  tall, og det `QualityPage` skriver for «God». */
export const FALLBACK_BITRATE_KBPS = 256;

/** Les bitraten ut av innstillingene. Samme regel som `app/state/disk.ts`:
 *  et ubrukelig tall faller tilbake på 256 i stedet for på 0. */
export function bitrateKbps(value: unknown): number {
  const n = parseInt(String(value ?? FALLBACK_BITRATE_KBPS), 10);
  return Number.isFinite(n) && n > 0 ? n : FALLBACK_BITRATE_KBPS;
}

/** Det fila selv er, slik `editor_load_recording` beskriver den. */
export interface SourceAudio {
  channels: number | null;
  sampleRate: number | null;
}

/**
 * Kilobit per sekund det VALGTE formatet kommer til å bruke.
 *
 * Samme form som `kbpsFor` i `app/state/disk.ts` — og med vilje samme tall der
 * de overlapper, for de svarer på det samme spørsmålet fra hver sin ende
 * («hvor mye plass trenger opptaket» / «hvor stor blir eksporten»). Forskjellen
 * er hvor tallene kommer fra: der leses de av INNSTILLINGENE, her av FILA. Å
 * estimere en eksport av et 96 kHz-opptak med opptaksinnstillingens 48 kHz
 * ville bommet med det dobbelte.
 */
export function exportKbps(
  format: ExportFormat,
  source: SourceAudio,
  mp3Bitrate: number,
): number {
  const stereo = (source.channels ?? 2) >= 2;
  if (format === "wav") {
    const rate =
      Number.isFinite(source.sampleRate) && (source.sampleRate ?? 0) > 0
        ? (source.sampleRate as number)
        : 48_000;
    return Math.round((rate * (stereo ? 2 : 1) * 16) / 1000);
  }
  // FLAC komprimerer, men hvor mye avhenger av materialet. Tallene er legacys
  // eget anslag (`loadDiskSpace` i `pages/home.ts`): rundt halvparten av WAV.
  if (format === "flac") return stereo ? 600 : 350;
  return mp3Bitrate;
}

/**
 * Anslått filstørrelse i byte.
 *
 * `kbps · 125` er byte per sekund (1000 bit / 8) — samme regnestykke som
 * disk-anslaget, og det er meningen: to tall om det samme skal ikke være regnet
 * ut på to måter.
 *
 * ⚠️ Bare for LYD. En video-eksport koder om bildet, og bitraten der avhenger
 * av oppløsning, bevegelse og x264s egne valg. Et tall vi ikke kan regne ut er
 * et tall vi ikke skal vise.
 */
export function estimatedBytes(keptSec: number, kbps: number): number | null {
  if (!Number.isFinite(keptSec) || keptSec <= 0) return null;
  if (!Number.isFinite(kbps) || kbps <= 0) return null;
  return Math.round(keptSec * kbps * 125);
}

/**
 * «27 MB» — megabyte, avrundet, uten desimaler over 10 og med én under.
 *
 * Ingen GB-trinn: en times gudstjeneste i WAV er ~600 MB, og «0,6 GB» er
 * vanskeligere å veie mot «har jeg plass» enn «600 MB». Ingen i18n her — «MB»
 * er en enhet, ikke prosa (samme regel som `dBFS` i S1b).
 */
export function megabytes(bytes: number | null): number | null {
  if (bytes === null || !Number.isFinite(bytes) || bytes <= 0) return null;
  const mb = bytes / 1_000_000;
  return mb < 10 ? Math.round(mb * 10) / 10 : Math.round(mb);
}

/**
 * Navnet eksporten kommer til å få.
 *
 * ⚠️ Dette er en FORUTSIGELSE, ikke en beslutning. Bakenden eier navnet:
 * `<stem>_redigert.<ext>` i mappen, og `collision_free_path` legger på `_2`,
 * `_3` … hvis det allerede ligger en fil der. Vi kan ikke vite om det gjør det,
 * så kvitteringen etter eksporten viser stien bakenden faktisk svarte med — den
 * er fasiten, denne er forhåndsvisningen.
 *
 * Canvasens «2026-08-23 Gudstjeneste – preken.mp3» ble IKKE bygget: `_redigert`
 * er sant uansett hva brukeren gjorde, og «– preken» ville vært en påstand om
 * innholdet i en fil der brukeren kanskje trykket «Behold alt».
 */
export function predictedOutputName(inputPath: string, ext: string): string {
  const name = inputPath.split(/[/\\]/).pop() ?? inputPath;
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  return `${stem}_redigert.${ext}`;
}

/** Mappen «Samme mappe som opptaket» peker på: opptakets egen. */
export function folderOf(inputPath: string): string {
  const at = Math.max(inputPath.lastIndexOf("/"), inputPath.lastIndexOf("\\"));
  return at > 0 ? inputPath.slice(0, at) : "";
}

/** Siste leddet i en sti — det er mappen brukeren kjenner igjen. */
export function folderLabel(folder: string): string {
  const trimmed = folder.replace(/[/\\]+$/, "");
  return trimmed.split(/[/\\]/).pop() || trimmed;
}

/**
 * Bakendens feilkode → SUFFIKSET under `editor.` som forklarer den.
 *
 * Suffikset og ikke hele nøkkelen: flaten slår det opp med
 * `tDyn("editor", suffix)`, fordi `check-i18n-keys.mjs` må ha et LITERALT
 * prefiks å slå opp — den samme formen `loadPhase` bruker i P4a.
 *
 * ⚠️ Lista SPEILER `EXPORT_ERROR_CODES` i
 * `legacy/renderer/pages/editor/export.ts`, som selv er grep-verifisert mot en
 * ekte emitter i Rust-sømmen for hver eneste rad. Grunnen til at den ikke bare
 * importeres er at legacys `describeExportError` bor i en modul som drar med
 * seg modal-manager, toast, mikser og legacys `E` — hele det gamle skallet, for
 * én tabell. Speilet er node-testet mot de samme kodene, og fase B slår dem
 * sammen igjen.
 *
 * Matches på den STABILE ledende koden (`errorCode`, R3-C): `AppError`
 * serialiseres som «<kategori>: <kode>[: detalj]», og et `includes`-søk over
 * hele meldingen ville truffet på prosa som bare NEVNER et kodeord.
 */
const EXPORT_ERROR_KEYS: ReadonlyArray<readonly [string, string]> = [
  ["no_audio_remaining", "errNoAudioRemaining"],
  ["cancelled", "errCancelled"],
  ["timeout", "errTimeout"],
  ["file_not_found", "errFileNotFound"],
  ["invalid_duration", "errCutData"],
  ["invalid_format", "errInvalidFormat"],
  ["path must be absolute", "errPathNotAbsolute"],
];

/** `null` = ingen kjent kode, og da sier flaten sin egen generelle setning
 *  heller enn å male en råstreng fra en annen prosess. */
export function exportErrorKey(err: string | undefined): string | null {
  const lead = errorCode(err);
  const hit =
    EXPORT_ERROR_KEYS.find(([code]) => code === lead) ??
    (err ? EXPORT_ERROR_KEYS.find(([code]) => err.includes(code)) : undefined);
  return hit ? hit[1] : null;
}

/** Var det brukeren som avbrøt? Da er det ikke en feil, og flaten skal si
 *  «Eksport avbrutt.» uten det røde. */
export function isCancelled(err: string | undefined): boolean {
  return exportErrorKey(err) === "errCancelled";
}
