/**
 * Diagnosens rene halvdel: koden → nøkkelen, og fakta → de fem statusradene.
 *
 * ## Hvorfor motorens tekst ikke er UI-tekst
 *
 * `sundayrec_core::diagnostics::detect_issues` skriver funnene sine på
 * HARDKODET NORSK — «Ingen lydenhet funnet», «Sjekk at lydkortet er tilkoblet
 * og driveren installert». En engelsk frivillig fikk altså norsk teknisk
 * sjargong i den ene visningen som finnes for å forklare hva som er galt.
 *
 * Motoren sender også en STABIL KODE (`SR-AUDIO-01`, `REC-LOSS`, …), og den er
 * kontrakten: koden betyr det samme på tvers av versjoner, så appen kan
 * oversette PÅ den. Regelen er `banners.ts`/`RecordPage`s, ordrett:
 *
 *   • en kode katalogen KJENNER → husets egen setning, på brukerens språk,
 *   • en kode katalogen IKKE kjenner → motorens egen prosa (`title`/`hint`).
 *
 * Steg 2 er ikke en unnskyldning, det er designet: en ny kode fra en nyere
 * bakende skal gi en SANN setning på feil språk, ikke stillhet eller en
 * generisk «ukjent feil». Stillhet er nøyaktig det denne kanalen produserte
 * mens skjermen ikke fantes.
 *
 * ## ⚠️ `detail` oversettes IKKE, og det er et bevisst hull
 *
 * De tre tekstfeltene er ikke like. `title` og `hint` er faste setninger i
 * Rust — ren UI-tekst, som hører hjemme i katalogen. `detail` er FAKTA satt
 * sammen med `format!`: enhetsnavnet innstillingen peker på, hvor mange GB som
 * er ledig, «15,3 % av lyden mangler». De tallene finnes ikke andre steder i
 * rapporten enn i den ferdig formaterte setningen, så å oversette dem ville
 * krevd at motoren sendte råverdiene ved siden av — en kontraktsendring i
 * Rust, som denne runden ikke gjør.
 *
 * Så: tittelen og rådet er på brukerens språk, faktalinja er motorens egen og
 * rendres som det den er (diagnostikk, ikke UI-tekst). Gjelden er notert til
 * språkrunden.
 *
 * ## Hvorfor det er en tabell og ikke en `switch`
 *
 * Fordi tabellen kan TESTES mot Rust-kilden: `diagnose-core.test.ts` har en rad
 * per kode, og en kode som legges til i `detect_issues` uten en rad her faller
 * på fallbacken i stedet for å ta ned siden. En `switch` med 20 grener spredt
 * i en komponent hadde vært den samme kunnskapen på et sted ingen gate ser.
 */

/**
 * Hver kode `sundayrec_core::diagnostics::detect_issues` kan sende, og
 * nøkkelsuffikset den oversettes med under `app.diagnose.f`.
 *
 * ⚠️ Kilden er `crates/sundayrec-core/src/diagnostics.rs` — 19 `SR-*`-koder
 * pluss `REC-LOSS` (varighetstapet, som med vilje har sitt eget navnerom fordi
 * det kommer fra `selftest`, ikke fra enumereringen). Legges en kode til der,
 * er den riktige rekkefølgen: rad her → nøkler i no.json + en.json → rad i
 * tabelltesten.
 */
export const FINDING_SLUGS: Readonly<Record<string, string>> = Object.freeze({
  "SR-FFMPEG-01": "ffmpeg01",
  "SR-AUDIO-01": "audio01",
  "SR-AUDIO-02": "audio02",
  "SR-AUDIO-10": "audio10",
  "SR-RATE-01": "rate01",
  "SR-VIDEO-01": "video01",
  "SR-VIDEO-02": "video02",
  "SR-DISK-01": "disk01",
  "SR-DISK-02": "disk02",
  "SR-PERM-01": "perm01",
  "SR-PERM-02": "perm02",
  "SR-ENGINE-01": "engine01",
  "SR-CAPTURE-01": "capture01",
  "SR-CAPTURE-02": "capture02",
  "SR-CRASH-01": "crash01",
  "SR-TASK-01": "task01",
  "SR-LOG-01": "log01",
  "SR-LOG-02": "log02",
  "SR-OK": "ok",
  "REC-LOSS": "recLoss",
});

/**
 * Nøkkelsuffikset for én kode, eller `null` for en kode katalogen ikke kjenner.
 *
 * `null` er svaret som utløser motorens prosa, og derfor er det et EKSPLISITT
 * svar og ikke en tom streng: kallstedet skal ta stilling til fallbacken, ikke
 * slå opp `app.diagnose.f..title` og få tom tekst.
 */
export function findingSlug(code: string): string | null {
  return FINDING_SLUGS[code] ?? null;
}

// ── De fem statusradene ──────────────────────────────────────────────────────

/** Radene, i den rekkefølgen legacy-modalen viste dem. */
export type StatusRowId = "devices" | "selected" | "mic" | "engine" | "probe";

/**
 * Fasiten en rad tegnes med. `null` er en TREDJE tilstand og ikke «nei»:
 * «kan ikke avgjøres» og «avgjort feil» er to forskjellige svar, og en rad som
 * viser ✕ for det første lyver om noe den ikke vet.
 */
export type RowTone = boolean | null;

export interface StatusRow {
  id: StatusRowId;
  tone: RowTone;
  /**
   * Nøkkelsuffikset verdien rendres fra, under `app.diagnose.v`. `null` når
   * verdien er RÅDATA (et enhetsnavn, et antall, en ffmpeg-versjon) — det er
   * fakta fra maskinen og hører ikke hjemme i en katalog.
   */
  valueSlug: string | null;
  /** Rådataen, når raden har en. Vises som den er. */
  valueText: string | null;
}

/**
 * Fakta de fem radene avledes av — plukket fra fire kilder som hver kan svare
 * «vet ikke», og derfor nullbare hele veien.
 */
export interface DiagnoseFacts {
  /** Lydinngangene `diagnose_audio` fant (`dshow`-lista). */
  inputs: readonly string[];
  /** Enheten innstillingen peker på, eller `null` for «standardenhet». */
  storedDevice: string | null;
  /** `media_permissions.microphone` — en `AuthStatus`, eller `null`. */
  micStatus: string | null;
  /** `ffmpeg_health`, eller `null` når proben ikke svarte. */
  ffmpeg: { available: boolean; version: string | null } | null;
  /** `DiagnosticsReport.captureOk` — `null` = proben kjørte ikke. */
  captureOk: boolean | null;
  /** `DiagnosticsReport.captureProbeSkipped` — motorens grunn, når den finnes. */
  probeSkipped: string | null;
}

/**
 * Er den LAGREDE enheten blant dem opptakeren ser?
 *
 * Sammenlikningen er legacys uklare navnetreff, og det er med vilje: opptakeren
 * adresserer enheten på nøyaktig samme måte (`diagnostics.rs` gjør
 * `eq_ignore_ascii_case` / `contains` begge veier), så en rad som krevde
 * eksakt streng ville sagt «ikke funnet» om en enhet motoren finner helt fint.
 * En diagnose som er strengere enn tingen den diagnostiserer er verre enn
 * ingen diagnose.
 *
 * Åtte tegn er legacys nål. Kort nok til å overleve at Windows henger
 * «(2- Qu-5)» foran navnet, langt nok til at «USB» ikke treffer alt.
 */
export function storedDeviceFound(
  stored: string,
  inputs: readonly string[],
): boolean {
  const needle = stored.toLowerCase().slice(0, 8);
  if (!needle) return false;
  return inputs.some(
    (n) =>
      n.toLowerCase().includes(needle) ||
      stored.toLowerCase().includes(n.toLowerCase().slice(0, 8)),
  );
}

/**
 * Om mikrofontilgangen er et JA, et NEI, eller ikke til å avgjøre.
 *
 * `unknown` og `notDetermined` er `null`, ikke `false`: den første er en
 * plattform som ikke kan svare (Windows), den andre er «ingen har spurt ennå,
 * første opptak utløser spørsmålet». Begge ville sett ut som en feil med et ✕.
 */
export function micOk(status: string | null): RowTone {
  if (status === "authorized") return true;
  if (status === "denied" || status === "restricted") return false;
  return null;
}

/** `AuthStatus`-verdiene katalogen har en setning for. */
const AUTH_SLUGS: readonly string[] = [
  "authorized",
  "denied",
  "restricted",
  "notDetermined",
];

/**
 * `AuthStatus` → nøkkelsuffiks, med `unknown` som gulv.
 *
 * Gulvet er det som gjør `tDyn` trygg her: den kaster i DEV på et oppslag som
 * bommer, og en `AuthStatus` vi ikke kjenner (en nyere bakende, en tom streng
 * fra en feilet probe) skal gi «kan ikke avgjøres» — ikke ta ned skjermen som
 * skulle forklart hva som var galt.
 */
export function authSlug(status: string | null | undefined): string {
  return status && AUTH_SLUGS.includes(status) ? status : "unknown";
}

/**
 * De fem radene, i rekkefølge.
 *
 * Radene er alltid FEM. En rad som forsvinner når den ikke har noe å si er en
 * liste som ser forskjellig ut hver gang, og da kan ingen lære hvor svaret
 * står — legacy droppet «valgt enhet» når ingen var valgt, og det var nettopp
 * i det tilfellet man lurte.
 */
export function statusRows(facts: DiagnoseFacts): StatusRow[] {
  const inputs = facts.inputs;
  const stored = (facts.storedDevice ?? "").trim();

  const found = stored ? storedDeviceFound(stored, inputs) : false;
  const selected: StatusRow = stored
    ? {
        id: "selected",
        tone: found,
        // Navnet står ALLTID; «ikke funnet» er et tillegg til det, ikke i
        // stedet for det. Å bytte ut navnet med en feilmelding fjerner den ene
        // opplysningen som gjør feilen mulig å rette.
        valueSlug: found ? null : "notFound",
        valueText: stored,
      }
    : // Ingen lagret enhet er ikke en feil — det er «standardenheten», som er
      // et gyldig og vanlig oppsett. Derfor `null`, ikke ✕.
      {
        id: "selected",
        tone: null,
        valueSlug: "defaultDevice",
        valueText: null,
      };

  const engine: StatusRow = facts.ffmpeg
    ? {
        id: "engine",
        tone: facts.ffmpeg.available,
        valueSlug: facts.ffmpeg.available
          ? facts.ffmpeg.version
            ? null
            : "ffmpegOk"
          : "ffmpegMissing",
        valueText: facts.ffmpeg.available ? facts.ffmpeg.version : null,
      }
    : { id: "engine", tone: null, valueSlug: "unknown", valueText: null };

  // Proben er tre-tilstands helt ned: den kan ha KJØRT og fått lyd, kjørt og
  // fått stillhet, eller ikke kjørt i det hele tatt. Den tredje sier motoren
  // hvorfor (`captureProbeSkipped`), og den grunnen vises ÆRLIG ved siden av —
  // en rad som bare sto tom ville lest som «alt i orden».
  const probe: StatusRow =
    facts.captureOk === null
      ? { id: "probe", tone: null, valueSlug: "probeSkipped", valueText: null }
      : {
          id: "probe",
          tone: facts.captureOk,
          valueSlug: facts.captureOk ? "probeOk" : "probeFail",
          valueText: null,
        };

  return [
    {
      id: "devices",
      tone: inputs.length > 0,
      valueSlug: null,
      valueText: String(inputs.length),
    },
    selected,
    {
      id: "mic",
      tone: micOk(facts.micStatus),
      valueSlug: `auth.${authSlug(facts.micStatus)}`,
      valueText: null,
    },
    engine,
    probe,
  ];
}

// ── Test-opptaket ────────────────────────────────────────────────────────────

/** `TestRecordingError`-variantene, som nøkkelsuffiks under `app.diagnose.testErr`. */
const TEST_ERROR_SLUGS: Readonly<Record<string, string>> = Object.freeze({
  device_not_found: "deviceNotFound",
  device_permission_denied: "permissionDenied",
  ffmpeg_error: "ffmpegError",
  no_audio: "noAudio",
});

/** `TestRecordingSignal`-variantene, under `app.diagnose.testSig`. */
const TEST_SIGNAL_SLUGS: Readonly<Record<string, string>> = Object.freeze({
  silent: "silent",
  low: "low",
  normal: "normal",
});

/**
 * Feilkoden fra et test-opptak → nøkkelsuffiks, eller `null`.
 *
 * Samme kontrakt som funnene: en variant vi ikke kjenner skal ikke gi tom
 * tekst. Kallstedet viser den RÅ koden da — «ffmpeg_error» er stygt, men det
 * er noe en frivillig kan lese opp i telefonen, og det er hele poenget med at
 * kodene er stabile.
 */
export function testErrorSlug(error: string | null | undefined): string | null {
  return (error && TEST_ERROR_SLUGS[error]) ?? null;
}

/** Signalstyrken → nøkkelsuffiks, eller `null` for en variant vi ikke kjenner. */
export function testSignalSlug(
  signal: string | null | undefined,
): string | null {
  return (signal && TEST_SIGNAL_SLUGS[signal]) ?? null;
}
