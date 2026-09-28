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

/**
 * Fasen FØR bakenden har hørt om eksporten i det hele tatt.
 *
 * De to andre fasekodene (`measuring`, `encoding`) er BAKENDENS, festet mot
 * Rust-siden av `export_phase_codes_match_the_renderer_literals`. Denne er
 * skallets egen, og den finnes fordi F2-2s vindu er ekte: `runExport` venter på
 * kanalanalysen — en full `astats`-passering, 30–60 s på en 90-minutters
 * gudstjeneste — FØR det finnes en ffmpeg å melde prosent for. Fram til F2-A-B
 * var det vinduet en knapp som så uberørt ut, og et andre klikk der var hele
 * dobbelteksporten.
 *
 * Den bor i det samme signalet som bakendens koder, ikke i et eget: flaten
 * spør «hva gjør eksporten nå», og det er ETT spørsmål med tre svar. Prisen er
 * at koden må være en Rust ALDRI sender — derav «preparing», som ingen av
 * ffmpeg-passeringene heter.
 */
export const EXPORT_PHASE_PREPARING = "preparing";

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
 * ⚠️ Dette er en FORUTSIGELSE, ikke en beslutning. Bakenden eier navnet
 * (`sundayrec_core::editor::export_stem`), og `collision_free_path` legger på
 * `_2`, `_3` … hvis det allerede ligger en fil der. Vi kan ikke vite om det gjør
 * det, så kvitteringen etter eksporten viser stien bakenden faktisk svarte med
 * — den er fasiten, denne er forhåndsvisningen.
 *
 * Uten tittel: `<stem>_redigert.<ext>`, som alltid. Med tittel:
 * `<YYYY-MM-DD> <tittel>.<ext>` — navnet et opplastingsskjema (SoundCloud og
 * de andre) foreslår som episodens tittel og adresse. Canvasens
 * «2026-08-23 Gudstjeneste – preken.mp3» ble ikke bygget fordi «– preken» ville
 * vært APPENS påstand om innholdet; en tittel er brukerens egne ord, og da er
 * navnet ingen gjetning.
 */
export function predictedOutputName(
  inputPath: string,
  ext: string,
  title: string | null = null,
  date: string | null = null,
): string {
  const name = inputPath.split(/[/\\]/).pop() ?? inputPath;
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  return `${exportStem(stem, title, date)}.${ext}`;
}

/** Lengste tittel, i tegn, et filnavn får bære. Samme tall som
 *  `EXPORT_TITLE_MAX_CHARS` i kjernen. */
export const EXPORT_TITLE_MAX_CHARS = 100;

/** Windows' reserverte enhetsnavn — `WIN_RESERVED` i `filename.rs`. */
const WIN_RESERVED = new Set([
  "CON",
  "PRN",
  "AUX",
  "NUL",
  ...Array.from({ length: 9 }, (_, i) => `COM${i + 1}`),
  ...Array.from({ length: 9 }, (_, i) => `LPT${i + 1}`),
]);

/**
 * Speilet av `export_stem` i kjernen, tegn for tegn.
 *
 * Begge leser vektorene i `crates/sundayrec-core/tests/fixtures/export-stem.json`
 * (`export-core.test.ts` her, `export_stem_matches_the_shared_vectors` der), så
 * forhåndsvisningen og fila ikke kan gli fra hverandre uten at en test går rød.
 * Tegn telles som Unicode-kodepunkter (`Array.from`), fordi Rusts `chars()`
 * gjør det — `.length` ville talt en emoji som to.
 */
export function exportStem(
  sourceStem: string,
  title: string | null,
  date: string | null,
): string {
  const untitled = `${sourceStem}_redigert`;
  if (title === null) return untitled;
  const cleaned = cleanTitle(title);
  if (cleaned.replace(/[. ]+$/u, "") === "") return untitled;
  const d = date?.trim() ?? "";
  return sanitizeFilename(isIsoDate(d) ? `${d} ${cleaned}` : cleaned);
}

function cleanTitle(title: string): string {
  const spaced = Array.from(title)
    .map((c) => (/[\p{Cc}\p{White_Space}]/u.test(c) ? " " : c))
    .join("");
  const collapsed = spaced.split(" ").filter(Boolean).join(" ");
  return Array.from(collapsed)
    .slice(0, EXPORT_TITLE_MAX_CHARS)
    .join("")
    .replace(/ +$/u, "");
}

/** `sanitize_filename` i `filename.rs`, portet. */
function sanitizeFilename(name: string): string {
  let safe = name.replace(/[/\\:*?"<>|]/gu, "_").trim();
  safe = safe.replace(/[. ]+$/u, "");
  // ASCII-only, som `eq_ignore_ascii_case`: `toUpperCase` alene gjør «ı» til «I».
  const ascii = safe.replace(/[a-z]/gu, (c) => c.toUpperCase());
  if (WIN_RESERVED.has(ascii)) safe = `_${safe}`;
  return safe === "" ? "opptak" : safe;
}

/** Nøyaktig `YYYY-MM-DD`, og en dato som finnes. */
export function isIsoDate(d: string): boolean {
  if (!/^\d{4}-\d{2}-\d{2}$/u.test(d)) return false;
  const [y, m, day] = d.split("-").map(Number) as [number, number, number];
  const probe = new Date(Date.UTC(y, m - 1, day));
  return (
    probe.getUTCFullYear() === y &&
    probe.getUTCMonth() === m - 1 &&
    probe.getUTCDate() === day
  );
}

/**
 * Opptaksdatoen som `YYYY-MM-DD` i LOKAL tid, eller `null` når vi ikke vet
 * når opptaket startet (en fil åpnet utenfra biblioteket).
 *
 * Lokal og ikke UTC: en gudstjeneste kl. 00:30 hører til den dagen menigheten
 * var i kirken, og opptakets eget filnavn (`local_date_str` i kjernen) regner
 * på samme måte.
 */
export function localIsoDate(ms: number | null): string | null {
  if (ms === null || !Number.isFinite(ms)) return null;
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** «Innhold» slik det lagres i opptakets `.meta.json`. */
export interface ExportContent {
  title: string;
  speaker: string;
  description: string;
}

/**
 * Les `.meta.json` tolerant: tre strenger, og alt annet ignoreres.
 *
 * Sidevogna kan være skrevet av en eldre versjon (Electron la `chapters` der)
 * eller redigert for hånd, og en fil vi ikke forstår er ikke en grunn til å
 * ikke åpne opptaket. `null` når det ikke er noe å hente.
 */
export function parseSavedContent(value: unknown): ExportContent | null {
  if (!value || typeof value !== "object") return null;
  const o = value as Record<string, unknown>;
  const str = (v: unknown) => (typeof v === "string" ? v : "");
  const content = {
    title: str(o.title),
    speaker: str(o.speaker),
    description: str(o.description),
  };
  return content.title || content.speaker || content.description
    ? content
    : null;
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
 * `disk_full` klassifiseres fra ffmpegs stderr i Rust (`run_export_ffmpeg`),
 * med det SAMME mønsteret opptakeren allerede matcher stderr mot
 * (`sundayrec_core::errors::classify_recording_error`) — «no space left»,
 * «disk quota exceeded» … kommer hit nøyaktig som de gjør midt i et opptak.
 * `cannot resolve path` er teksten `path_guard::checked_input_file` gir når
 * kildefila er borte — den vakten kjører FØR `export()` selv rekker å si
 * `file_not_found`, så uten denne rada var den vanligste måten en fil
 * forsvinner på (frakoblet disk, flyttet/slettet fil) usynlig for tabellen.
 *
 * `export_already_running` er bakendens enkelt-flyt-vakt (F2-A-B,
 * `ExportEngine::try_begin`). Den skal i praksis aldri nå en frivillig — skallet
 * setter `exporting` FØR kanalanalysen nå, så knappen rekker ikke å bli klikket
 * to ganger — men vakten står i Rust nettopp fordi «flaten ville aldri gjort
 * det» var antakelsen som brakk. En vakt uten en setning er en dialogboks med
 * råtekst fra en annen prosess.
 *
 * `disk_low_for_export` er diskvakten FØR renderen (F2-11). Den er ikke det
 * samme som `disk_full`: den ene sier «dette får ikke plass» før du har ventet i
 * tjue minutter, den andre er ffmpeg som gikk tom midtveis. Begge er sanne, men
 * bare den første kommer i tid til å være til nytte — derfor har de hver sin
 * setning.
 *
 * Matches på den STABILE ledende koden (`errorCode`, R3-C): `AppError`
 * serialiseres som «<kategori>: <kode>[: detalj]». Fallback-søket under bruker
 * `includes`, men KUN for kodene som har et mellomrom i seg — fraser som
 * ALDRI kan bli `lead`, siden den ledende-kode-regexen aldri fanger mer enn
 * ett `[a-z0-9_]`-ord. Et ett-ords kode som `timeout`/`cancelled` matcher
 * ALDRI via fallback: et rått `includes`-søk over en hel ffmpeg-stderr-hale
 * (opptil 500 tegn rå prosa) ville truffet ordet «timeout» i en helt
 * urelatert nettverksklage og løyet om hvorfor eksporten faktisk stoppet.
 */
const EXPORT_ERROR_KEYS: ReadonlyArray<readonly [string, string]> = [
  ["no_audio_remaining", "errNoAudioRemaining"],
  ["cancelled", "errCancelled"],
  ["timeout", "errTimeout"],
  ["file_not_found", "errFileNotFound"],
  ["disk_full", "errDiskFull"],
  ["invalid_duration", "errCutData"],
  // Reparasjonen leser høyre inngangskanal på en fil som ikke har en. ffmpeg
  // avviser den IKKE — den gjengir 6 dB ned uten et ord — så sømmen stopper
  // eksporten, og da må det stå hvorfor.
  ["channel_repair_needs_stereo", "errChannelRepairNeedsStereo"],
  ["invalid_format", "errInvalidFormat"],
  ["export_already_running", "errExportAlreadyRunning"],
  ["disk_low_for_export", "errDiskLowForExport"],
  ["path must be absolute", "errPathNotAbsolute"],
  ["cannot resolve path", "errFileNotFound"],
];

/** `null` = ingen kjent kode, og da sier flaten sin egen generelle setning
 *  heller enn å male en råstreng fra en annen prosess. */
export function exportErrorKey(err: string | undefined): string | null {
  const lead = errorCode(err);
  const hit =
    EXPORT_ERROR_KEYS.find(([code]) => code === lead) ??
    (err
      ? EXPORT_ERROR_KEYS.find(
          ([code]) => code.includes(" ") && err.includes(code),
        )
      : undefined);
  return hit ? hit[1] : null;
}

/** Var det brukeren som avbrøt? Da er det ikke en feil, og flaten skal si
 *  «Eksport avbrutt.» uten det røde. */
export function isCancelled(err: string | undefined): boolean {
  return exportErrorKey(err) === "errCancelled";
}
