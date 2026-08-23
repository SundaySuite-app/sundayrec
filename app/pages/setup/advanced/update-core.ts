/**
 * Oppdateringsraden som en TILSTANDSMASKIN, ren og tabelltestet.
 *
 * ## Hvorfor dette ikke kan bo i komponenten
 *
 * Legacy maler den samme raden fra sju forskjellige hendelseslyttere
 * (`update-checking`, `-available`, `-download-progress`, `-downloaded`,
 * `-restarting`, `-error`, `-not-available`), og hver av dem skriver tre
 * steder: prikken, teksten og KNAPPENE. Feilen som fantes der i to utgivelser
 * var nettopp en skjøt mellom to av dem: «Start på nytt og installer» ble
 * stående etter en «du er oppdatert», altså en knapp som lovet en
 * oppdatering som ikke fantes. Sju lyttere × tre skrivninger er tjueén steder
 * å glemme én.
 *
 * Her er det ÉN fase inn og ÉN visning ut. En fase kan ikke la en knapp fra
 * en tidligere fase bli stående, fordi visningen bygges fra bunnen hver gang.
 *
 * Ingen i18n her: kjernen svarer med en NØKKEL og innsettingene, kallstedet
 * oversetter. (Samme regel som `decisions-core.ts`.)
 */

/** Hvor i løpet vi er. `idle` er «ingen har spurt ennå». */
export type UpdatePhase =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "upToDate" }
  | { kind: "available"; version: string }
  | { kind: "downloading"; percent: number }
  | { kind: "ready"; version: string }
  | { kind: "restarting" }
  | { kind: "failed"; restartFailed: boolean };

/** Hva raden skal si, som data. */
export interface UpdateView {
  /** Katalognøkkel-suffikset under `app.setup.advanced.` — eller `null` for
   *  «si ingenting», som er den ærlige tilstanden før noen har spurt. */
  message:
    | null
    | { key: "updateChecking" }
    | { key: "updateUpToDate" }
    | { key: "updateAvailable"; version: string }
    | { key: "updateDownloading"; percent: number }
    | { key: "updateReady"; version: string }
    | { key: "updateRestarting" }
    | { key: "updateRestartFailed" }
    | { key: "updateFailed" };
  /** Tonen meldingen har. `null` = ingen melding. */
  tone: "neutral" | "good" | "warn" | "bad" | null;
  /**
   * Handlingsknappen, eller `null` når det ikke er noe å gjøre.
   *
   * `download` og `install` er to FORSKJELLIGE handlinger med to
   * forskjellige etiketter — legacy brukte én knapp som byttet tekst tre
   * ganger, og det var der «trykk for å installere» kunne stå på noe som
   * ennå ikke var lastet ned.
   */
  action: null | { key: "download" | "install"; busy: boolean };
  /** Kan «Se etter oppdateringer nå» trykkes? */
  canCheck: boolean;
}

/**
 * Fase → visning. Total: hver fase svarer på alle fire spørsmålene, så en
 * knapp kan ikke overleve inn i en tilstand som ikke ba om den.
 */
export function updateView(phase: UpdatePhase): UpdateView {
  switch (phase.kind) {
    case "idle":
      return { message: null, tone: null, action: null, canCheck: true };
    case "checking":
      return {
        message: { key: "updateChecking" },
        tone: "neutral",
        action: null,
        canCheck: false,
      };
    case "upToDate":
      return {
        message: { key: "updateUpToDate" },
        tone: "good",
        // Ingen knapp. Dette er raden legacy lot stå med «Start på nytt og
        // installer» etter en oppdatert sjekk.
        action: null,
        canCheck: true,
      };
    case "available":
      return {
        message: { key: "updateAvailable", version: phase.version },
        tone: "warn",
        action: { key: "download", busy: false },
        canCheck: true,
      };
    case "downloading":
      return {
        message: { key: "updateDownloading", percent: phase.percent },
        tone: "neutral",
        action: { key: "download", busy: true },
        canCheck: false,
      };
    case "ready":
      return {
        message: { key: "updateReady", version: phase.version },
        tone: "warn",
        action: { key: "install", busy: false },
        canCheck: true,
      };
    case "restarting":
      return {
        message: { key: "updateRestarting" },
        tone: "neutral",
        action: { key: "install", busy: true },
        canCheck: false,
      };
    case "failed":
      return {
        message: phase.restartFailed
          ? { key: "updateRestartFailed" }
          : { key: "updateFailed" },
        tone: "bad",
        // En feilet omstart betyr at nedlastingen ER ferdig — knappen skal
        // kunne prøves igjen. En feilet SJEKK har ingenting å installere.
        action: phase.restartFailed ? { key: "install", busy: false } : null,
        canCheck: true,
      };
  }
}

/**
 * Kanalene shimmen syntetiserer, og hva de betyr.
 *
 * Tauris oppdaterer-kommandoer POLLES (`update_status`, `update_check`,
 * `update_download_install`, `update_relaunch`); api-shimmen gjør om svarene
 * til de sju Electron-hendelsene under, og eier både nedlastingsløkka og
 * dødmannsbryteren for «omstarten skjedde ikke». Vi lytter på hendelsene i
 * stedet for å polle selv: én maskin som driver løpet, ikke to som er nesten
 * enige.
 */
export const UPDATE_CHANNELS = [
  "update-checking",
  "update-available",
  "update-not-available",
  "update-download-progress",
  "update-downloaded",
  "update-restarting",
  "update-error",
] as const;

export type UpdateChannel = (typeof UPDATE_CHANNELS)[number];

/** Payloaden shimmen sender med, slik den faktisk ser ut. */
export interface UpdateEventPayload {
  version?: string;
  percent?: number;
}

/**
 * Hendelse → fase. `null` = en kanal vi ikke kjenner, som skal ignoreres og
 * ikke kaste inne i en event-callback der ingenting fanger det.
 *
 * `update-error` bærer en STRENG, ikke et objekt, og den ene verdien som betyr
 * noe er `restart_failed`: da er nedlastingen ferdig og knappen skal kunne
 * prøves igjen. Alt annet er en feilet sjekk, som ikke har noe å installere.
 */
export function phaseFromEvent(
  channel: string,
  payload: unknown,
): UpdatePhase | null {
  const data = (payload ?? {}) as UpdateEventPayload;
  switch (channel) {
    case "update-checking":
      return { kind: "checking" };
    case "update-available":
      return { kind: "available", version: data.version ?? "" };
    case "update-not-available":
      return { kind: "upToDate" };
    case "update-download-progress":
      return { kind: "downloading", percent: clampPercent(data.percent ?? 0) };
    case "update-downloaded":
      return { kind: "ready", version: data.version ?? "" };
    case "update-restarting":
      return { kind: "restarting" };
    case "update-error":
      return { kind: "failed", restartFailed: payload === "restart_failed" };
    default:
      return null;
  }
}

/** 0–100, hele tall. Bakenden klamrer sin egen `u8`, men et tall fra en
 *  polling-runde skal ikke kunne male en stolpe utenfor sporet. */
export function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, Math.round(value)));
}
