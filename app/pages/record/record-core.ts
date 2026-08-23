/**
 * Opptakssidens avgjørelser — som en tabell, ikke som `&&` inne i en JSX-linje.
 *
 * ## Den ene regelen hele sett 2 hviler på
 *
 * **Start er sperret til en lydkilde er valgt eksplisitt** — også når valget er
 * maskinens egen mikrofon. Det er den største adferdsendringen i «Frivilligen
 * først», og grunnen står i atlaset §3a: en fersk installasjon tar i dag opp på
 * laptop-mikrofonen uten å si fra, fordi `deviceId: null` betyr «systemets
 * standardinngang» for opptakeren og «Innebygd mikrofon · Tilkoblet ✓» for
 * skjermen. To sanne halvdeler, og en gudstjeneste tatt opp fra feil rom.
 *
 * En slik regel kan ikke bo i en komponent. Der er den én betingelse ingen
 * leser to ganger, og den er umulig å bevise uten å klikke. Her er den en ren
 * funksjon med en rad per tilstand — og `e2e/app/record.spec.ts` har en
 * mutasjonsprøve som fjerner sperren og forventer rødt.
 *
 * ## Tre tilstander, og hvorfor `null` ikke er den fjerde
 *
 *   `no-source`       ingenting er valgt          → Start SPERRET
 *   `source-missing`  valgt, men ikke funnet nå   → Start TILLATT, med advarsel
 *   `ready`           valgt og funnet             → Start TILLATT
 *
 * Enhetslisten leses ASYNKRONT etter første maling. `devices === null` betyr
 * «ikke sett etter ennå», og det er ikke bevis for at mikseren er borte — så
 * den faller tilbake på `ready` og påstår ingenting. Det motsatte ville gjort
 * hver kaldstart til et gult «Finner ikke Behringer X32» som blir borte igjen
 * etter 100 ms, og et gult kort som forsvinner av seg selv er nettopp det som
 * lærer folk å ignorere gult. Samme regel som `decisions-core`s `unknown` og
 * `soundChosen` i `state/devices.ts`.
 *
 * ## Hvorfor `source-missing` fortsatt får starte
 *
 * Canvasens 2.3: «Du kan starte likevel — opptaket blir stille til mikseren er
 * tilbake.» Et valg ER tatt; enheten er bare ikke der akkurat nå. Å sperre da
 * ville betydd at en USB-kabel som satt løst i to sekunder kostet hele
 * gudstjenesten, og opptakeren håndterer selv en enhet som kommer tilbake
 * (gjenkoblingspolicyen i capture-domenet).
 *
 * ## Formatering hører hjemme her, tidsSONER gjør det ikke
 *
 * Klokken, plassen og varigheten er ren aritmetikk og står her. Alt som er
 * avhengig av `Intl` (ukedag, dato) står IKKE her — samme grense som
 * `status-line.ts` trekker, og av samme grunn: en tabell skal ikke bli skjør
 * av hvilken ICU-versjon node eller WebKit er bygget med.
 */

import type { ChannelPair } from "../setup/decisions-core";
import { channelPairFor } from "../setup/decisions-core";
import type { Settings } from "../../state/settings";

/** De tre tilstandene kilde-kortet kan være i. */
export type SourceKind = "no-source" | "source-missing" | "ready";

/** Én lydenhet, slik denne kjernen trenger den. Strukturell med vilje: den
 *  samme formen `state/devices.ts` og `decisions-core` bruker. */
export interface DeviceFact {
  id: string;
  name: string;
  channels: number;
  isDefault: boolean;
}

export interface SourceState {
  kind: SourceKind;
  /** Enhets-id-en som står lagret. Tom streng når ingen er valgt. */
  deviceId: string;
  /** Navnet skjermen sier. Tom streng når ingen er valgt. */
  name: string;
  /**
   * Kanalparet, 1-indeksert — bare for enheter med flere enn to kanaler.
   * Et stereokort har ingen kanaler å velge mellom, og «kanal 1–2» på et
   * USB-mikrofon er en opplysning som bare kan misforstås.
   */
  pair: ChannelPair | null;
  /** Har Start lov til å gjøre noe? Se toppen av fila. */
  canStart: boolean;
}

/**
 * Hvilken tilstand kilde-kortet er i.
 *
 * `devices === null` = ikke lest ennå. Se toppen av fila.
 */
export function sourceState(
  settings: Settings,
  devices: readonly DeviceFact[] | null,
): SourceState {
  const deviceId = (settings.deviceId ?? "").trim();
  const stored = (settings.deviceName ?? "").trim() || deviceId;

  // Ingenting valgt. `deviceName` alene teller ikke: en profil kan bære et navn
  // fra en migrering uten at noen noensinne har tatt valget.
  if (!deviceId) {
    return {
      kind: "no-source",
      deviceId: "",
      name: "",
      pair: null,
      canStart: false,
    };
  }

  if (devices === null) {
    return {
      kind: "ready",
      deviceId,
      name: stored,
      pair: null,
      canStart: true,
    };
  }

  const hit = devices.find((d) => d.id === deviceId);
  if (!hit) {
    return {
      kind: "source-missing",
      deviceId,
      name: stored,
      pair: null,
      canStart: true,
    };
  }

  return {
    kind: "ready",
    deviceId,
    name: hit.name,
    pair: hit.channels > 2 ? channelPairFor(settings, deviceId) : null,
    canStart: true,
  };
}

/**
 * Vertens standardenhet — nødutgangen «Bruk maskinens mikrofon» (canvas 2.3).
 *
 * En EKTE enhet med en ekte id, aldri `deviceId: null`. Det er hele poenget:
 * nødutgangen skal ta valget på brukerens vegne, ikke gjøre det utatt igjen.
 * `null` her betyr at maskinen ikke rapporterte noen standardinngang, og da
 * finnes det ingen utgang å tilby.
 */
export function defaultDeviceOf(
  devices: readonly DeviceFact[] | null,
): DeviceFact | null {
  return devices?.find((d) => d.isDefault) ?? null;
}

// ── Formatering ─────────────────────────────────────────────────────────────

/**
 * Millisekunder → «0:42:17». Timer uten ledende null (canvasens 2.4), minutter
 * og sekunder med. Negativt regnes som null: en klokke som teller nedover fordi
 * to maskinklokker er uenige skal stå på 0:00:00, ikke vise et minustall.
 */
export function formatClock(ms: number): string {
  const total = Number.isFinite(ms) && ms > 0 ? Math.floor(ms / 1000) : 0;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return `${h}:${pad2(m)}:${pad2(s)}`;
}

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/**
 * Hvilken av de tre setningene et minutt-tall skal si, og med hvilke tall.
 *
 * ## Hvorfor en form og ikke en streng
 *
 * `tn()` er utelukket: `check-i18n-plurals.mjs` krever hver flertallsgruppe i
 * ALLE sju språk med riktige CLDR-kategorier og har ingen unntak for de fem
 * pausede — en ny `tn()`-nøkkel ville krevd polske flertallsformer midt i
 * pausen som finnes for å slippe akkurat det. Så: tre `tf()`-nøkler, og
 * kjernen velger hvilken. «14 t», «14 t 20 min» og «45 min» er riktige for
 * hele tallområdet de faktisk vises for.
 */
export type SpanKind = "none" | "hours" | "hoursMinutes" | "minutes";

export interface Span {
  kind: SpanKind;
  hours: number;
  /** Minuttene som blir til overs. Nullpolstret når timer står foran. */
  minutes: string;
}

/** Minutter → formen «14 t» / «14 t 20 min» / «45 min». */
export function spanOfMinutes(total: number | null): Span {
  if (total === null || !Number.isFinite(total) || total < 0) {
    return { kind: "none", hours: 0, minutes: "0" };
  }
  const whole = Math.floor(total);
  const h = Math.floor(whole / 60);
  const m = whole % 60;
  if (h === 0) return { kind: "minutes", hours: 0, minutes: String(m) };
  if (m === 0) return { kind: "hours", hours: h, minutes: "0" };
  return { kind: "hoursMinutes", hours: h, minutes: pad2(m) };
}

/** Sekunder → den samme formen. «1 t 02 min» for et opptak som varte så lenge. */
export function spanOfSeconds(total: number | null): Span {
  if (total === null || !Number.isFinite(total) || total < 0) {
    return { kind: "none", hours: 0, minutes: "0" };
  }
  return spanOfMinutes(Math.round(total / 60));
}

/**
 * Byte → «112 MB» / «1,4 GB».
 *
 * Enhetene er KONSTANTER og ikke katalognøkler: MB og GB heter det samme på
 * alle sju språk, og en enhet i katalogen er en enhet noen kommer til å
 * oversette. Desimaltegnet følger språket, fordi det ikke gjør det —
 * `toLocaleString` er den eneste delen her som er lokal.
 */
const MB = "MB";
const GB = "GB";

export function formatBytes(bytes: number | null, locale: string): string {
  if (bytes === null || !Number.isFinite(bytes) || bytes < 0) return "";
  const mb = bytes / 1e6;
  if (mb < 1000) {
    return `${Math.round(mb).toLocaleString(locale)} ${MB}`;
  }
  return `${(mb / 1000).toLocaleString(locale, {
    minimumFractionDigits: 1,
    maximumFractionDigits: 1,
  })} ${GB}`;
}

/** Filnavnet i en sti. Samme `basename` som api-shimmen bruker på historikken. */
export function basename(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/**
 * Stor forbokstav på første tegn.
 *
 * `Intl` gir «søndag 16. august» — riktig norsk midt i en setning, og feil
 * som det FØRSTE ordet i en overskrift. Bare første tegn røres, og bare med
 * `toLocaleUpperCase` for språket som gjelder: en generell «title case» ville
 * vært en regel som er gal i de fleste av appens språk.
 */
export function capitalizeFirst(text: string, locale: string): string {
  if (!text) return text;
  return text[0].toLocaleUpperCase(locale) + text.slice(1);
}

/**
 * Motorens feilkode → SUFFIKSET i `recording.*`-katalogen.
 *
 * Tabellen er den samme som `translateNativeError` i
 * `legacy/renderer/pages/recording.ts`, og den peker på de SAMME
 * katalognøklene. Ikke en kopi av tekstene: kopierte tekster driver fra
 * hverandre, og disse finnes allerede oversatt i alle sju språk. Grunnen til
 * at funksjonen likevel finnes her er at legacy-utgaven bor i en 1100-linjers
 * DOM-modul som `app/` ikke kan importere uten å dra hele det gamle treet med
 * seg — samme forbehold som `state/disk.ts` sitt.
 *
 * En ukjent kode lander på `errorUnknown` og logges. Å vise rå-koden ville
 * vært å be en frivillig om å oversette `device_not_found` selv.
 */
const NATIVE_ERRORS: Record<string, string> = {
  no_device: "errorDeviceNotFound",
  device_not_found: "errorDeviceNotFound",
  device_permission_denied: "errorPermission",
  device_busy: "errorNotReadable",
  device_error: "errorDeviceError",
  already_recording: "errorAlreadyRecording",
  empty_output: "errorEmpty",
  save_folder_permission: "errorFolderPermission",
  save_folder_error: "errorFolderError",
  device_disconnected: "errorDeviceDisconnected",
  disk_full: "errorDiskFull",
  ffmpeg_missing: "errorFfmpegMissing",
  stuck_recording: "errorStuck",
  invalid_opts: "errorInvalidOpts",
  no_save_folder: "errorNoSaveFolder",
};

export function nativeErrorSuffix(code: string | null | undefined): string {
  if (!code) return "errorUnknown";
  const hit = NATIVE_ERRORS[code];
  if (hit) return hit;
  console.warn("[record] ukjent feilkode fra motoren:", code);
  return "errorUnknown";
}

/**
 * Feilmeldingen `startRecordingNow` svarte med → det samme suffikset.
 *
 * Shimmen svarer `{ ok: false, error }` der `error` er `AppError`-ens
 * `message`, og den bærer koden. `ipcErrText` kan legge mer rundt den, så vi
 * leter etter en kjent kode i teksten i stedet for å kreve at hele strengen ER
 * koden — det var nettopp den antakelsen som ga «[object Object]» i den gamle
 * stien.
 */
export function nativeErrorSuffixFromText(text: string | null): string {
  if (!text) return "errorUnknown";
  const trimmed = text.trim();
  if (NATIVE_ERRORS[trimmed]) return NATIVE_ERRORS[trimmed];
  for (const code of Object.keys(NATIVE_ERRORS)) {
    if (trimmed.includes(code)) return NATIVE_ERRORS[code];
  }
  console.warn("[record] ukjent feiltekst fra motoren:", text);
  return "errorUnknown";
}
