/**
 * `backend://warning` — de fem tingene motoren roper om når ingen spurte.
 *
 * ## ⚠️ Kanalen hadde INGEN lytter
 *
 * Shimmen har kartlagt `'backend-warning' → 'backend://warning'` siden fase 2,
 * og Rust emitterer på den fra fem steder (`crate::notify::warn`, og — for
 * papirkurven, som ikke har noe `AppHandle` — `crate::notify::warn_detached`):
 * pre-roll ga opp, gjenopprettingen hoppet over en fil, den valgte lydenheten
 * er ikke tilkoblet, disken fylles, papirkurvens manifest var ulesbart.
 * Legacy-skallet hørte på den i `pages/home.ts` og reiste en toast. Byttet tok
 * med seg shimmen og lot lytteren bli igjen.
 *
 * Resultatet var den stilleste feilformen som finnes: bakenden SIER fra, hele
 * veien opp til nettleseren, og så er det ingen der. «Mikseren er ikke
 * tilkoblet», en halvtime før et planlagt opptak, gikk rett i gulvet.
 *
 * ## Banner, ikke toast
 *
 * Legacy valgte toast. Dette skallet velger banner, og av bannerkøens egen
 * grunn (`state/banners.ts`): «Something is wrong and stays wrong.» En
 * forhåndsbuffer som har gitt opp er ikke over om åtte sekunder, og en mikser
 * som ikke er i huset blir ikke tilkoblet av at meldingen forsvant. NØKLET per
 * kode, så en disk som fyller seg i ti runder oppdaterer ETT banner.
 *
 * ## Data, ikke tekst — også her
 *
 * Køen bærer koden og motorens `params`, aldri en ferdig setning. Siden
 * oversetter, med `notify.*`-nøklene legacy hadde i alle sju katalogene (de er
 * hentet tilbake ordrett — en tekst som allerede er oversatt til sju språk
 * skrives ikke på nytt). Er koden UKJENT for denne katalogen, brukes motorens
 * egen `msg` ordrett: en sann setning på feil språk er bedre enn stillhet, og
 * stillhet er nøyaktig det denne fila finnes for å avslutte.
 *
 * ## Dedupliseringsregelen — ett faktum, ett banner
 *
 * To av kodene har allerede en flate i skallet, og to bannere om det
 * samme er to setninger som kan bli uenige:
 *
 * - **`disk_low`** ⇢ opptakssidens `banner-low-disk`, avledet av
 *   `state/disk.ts` + `LOW_DISK_MINUTES`. Den sier det i MINUTTER MED PLASS,
 *   som er enheten en frivillig kan handle på; bakenden sier det i GB.
 * - **`device_missing`** ⇢ opptakssidens `record-source-missing`, avledet av
 *   `sourceState(settings, audioDevices)`. Den NAVNGIR enheten og tilbyr
 *   nødutgangen.
 *
 * Regelen er ikke «kast disse to». Den er: **advarselen oppdaterer den
 * eksisterende kilden, og reiser sitt eget banner bare når den kilden ikke
 * allerede står.** `disk_low` utløser en `refreshDiskSpace()` (ellers kan
 * skallets egen måling være opptil et minutt gammel), `device_missing` en
 * `loadAudioDevices()`. Står den avledede flaten etterpå, er saken sagt én
 * gang. Står den ikke — og det er den interessante halvdelen, for
 * `device_missing` kommer fra SCHEDULEREN før noen har åpnet opptakssiden, og
 * tersklene for disk er ulike (bakenden måler GB, skallet måler minutter, og
 * video spiser mangedobbelt per minutt) — så sier advarselen det selv. Å droppe
 * den der ville vært å gjeninnføre feilen i det ene tilfellet den koster mest.
 */

import type { BackendWarning } from "@legacy/bindings/BackendWarning";

import {
  dismissBanner,
  raiseBanner,
  type BackendWarningBanner,
  type BackendWarningKey,
} from "./banners";
import { currentRoomMinutes, refreshDiskSpace } from "./disk";
import { audioDevices, loadAudioDevices, soundChosen } from "./devices";
import { markPrerollDead } from "./preroll";
import { settings } from "./settings";
import { LOW_DISK_MINUTES } from "./status-line";

/**
 * Kode → bannernøkkel. Speiler `sundayrec_core::notify::code`; RUST er
 * fasiten, og `backend-warning.test.ts` leser `code::ALL` derfra og feiler hvis
 * denne tabellen ikke dekker den. En ny kode i Rust skal ikke kunne gli
 * gjennom og bli en tom setning her.
 */
export const WARNING_BANNER_KEYS: Record<string, BackendWarningKey> = {
  preroll_dead: "backend-preroll-dead",
  recovery_skipped: "backend-recovery-skipped",
  device_missing: "backend-device-missing",
  disk_low: "backend-disk-low",
  // F1-M2: papirkurvens manifest var ulesbart og ble flyttet til side.
  trash_manifest_unreadable: "backend-trash-manifest",
};

/** Katalognøkkelens suffiks under `notify.*` for en kjent kode. */
export const WARNING_SUFFIXES: Record<string, string> = {
  preroll_dead: "prerollDead",
  recovery_skipped: "recoverySkipped",
  device_missing: "deviceMissing",
  disk_low: "diskLow",
  // F1-M2.
  trash_manifest_unreadable: "trashManifestUnreadable",
};

/** Byte per GB, 1024³ — det samme tallet forhåndssjekken og disken bruker. */
const BYTES_PER_GB = 1_073_741_824;

/**
 * Interpolasjonsverdiene, med de avledede lagt til.
 *
 * `freeBytes` kommer som et eksakt bytetall (bakenden skal ikke gjette på
 * brukerens enhetsvaner); `freeGb` er det et menneske kan lese. Ordrett
 * legacys `warningParams`.
 */
export function warningParams(w: {
  params?: Record<string, string> | null;
}): Record<string, string> {
  const params: Record<string, string> = { ...(w.params ?? {}) };
  const free = Number(params.freeBytes);
  if (Number.isFinite(free) && free >= 0) {
    params.freeGb = (free / BYTES_PER_GB).toFixed(1);
  }
  return params;
}

/**
 * Sett inn hver `{navn}` vi har en verdi for. En ukjent plassholder blir
 * STÅENDE i stedet for å bli blanket: en synlig `{file}` er en feilrapport, en
 * stille tom setning er ikke. Legacys `interpolate`, ordrett.
 */
export function interpolate(
  template: string,
  params: Record<string, string>,
): string {
  return template.replace(/\{(\w+)\}/g, (whole, key: string) =>
    Object.prototype.hasOwnProperty.call(params, key) ? params[key] : whole,
  );
}

/** Det skallet allerede sier om det samme — se dedupliseringsregelen over. */
export interface ExistingSurfaces {
  /** Står opptakssidens `banner-low-disk`? */
  lowDiskShown: boolean;
  /** Står opptakssidens `record-source-missing`? */
  deviceMissingShown: boolean;
}

/** Hva som skal skje med én advarsel. */
export type WarningPlan =
  | { action: "raise"; banner: BackendWarningBanner }
  | { action: "deduped"; code: string }
  | { action: "ignore" };

/**
 * Avgjør hva én `backend://warning` skal bli. Ren — hele forgreningen er
 * testbar uten et vindu, en IPC eller et banner.
 *
 * Tolerant med formen med vilje: dette krysser en IPC-grense, og en advarsel
 * som kaster mens den forteller om et problem er verre enn problemet.
 */
export function planWarning(
  data: unknown,
  surfaces: ExistingSurfaces,
): WarningPlan {
  if (!data || typeof data !== "object") return { action: "ignore" };
  const w = data as Partial<BackendWarning>;
  const code = typeof w.code === "string" ? w.code : "";
  const msg = typeof w.msg === "string" && w.msg ? w.msg : null;
  // Verken en kode å slå opp eller en setning å vise: det finnes ingenting
  // sanne å si, og en tom stripe er en flate uten innhold.
  if (!code && !msg) return { action: "ignore" };

  if (code === "disk_low" && surfaces.lowDiskShown)
    return { action: "deduped", code };
  if (code === "device_missing" && surfaces.deviceMissingShown)
    return { action: "deduped", code };

  const key: BackendWarningKey = WARNING_BANNER_KEYS[code] ?? "backend-warning";

  return {
    action: "raise",
    banner: {
      key,
      code,
      msg,
      // Bakendens `WarnSeverity` er `warn | error`; alt annet leses som `warn`,
      // for en ukjent alvorlighetsgrad er ingen grunn til å rope høyere.
      severity: w.severity === "error" ? "error" : "warn",
      params: warningParams(w),
    },
  };
}

/**
 * Flatene skallet allerede har oppe, lest ut av signalene.
 *
 * Begge er satt sammen av `state/`s EGNE regler — `currentRoomMinutes()` +
 * `LOW_DISK_MINUTES`, og `soundChosen()` — og ikke skrevet på nytt her.
 * Opptakssiden avleder de samme to svarene av de samme to reglene
 * (`sourceState`s `source-missing`-gren er `soundChosen` med et valgt id foran
 * seg), og `backend-warning.test.ts` pinner at de to fortsatt er enige. To
 * kopier av «står den flaten?» er nøyaktig skjøten som gjør at den ene sier ja
 * og den andre nei — og da er vi tilbake til to bannere om det samme.
 */
export function currentSurfaces(): ExistingSurfaces {
  const room = currentRoomMinutes();
  const s = settings.value;
  return {
    lowDiskShown: room !== null && room < LOW_DISK_MINUTES,
    deviceMissingShown:
      (s.deviceId ?? "").trim() !== "" && !soundChosen(s, audioDevices.value),
  };
}

let dispose: (() => void) | null = null;

/**
 * Ta imot én advarsel: oppdater den eksisterende kilden, og avgjør DERETTER om
 * advarselen har noe eget å si.
 *
 * Rekkefølgen er hele dedupliseringsregelen. Å avgjøre først og oppdatere
 * etterpå ville gitt begge deler: et bakende-banner reist på en opptil et
 * minutt gammel diskmåling, og skallets eget avledede banner rett etterpå, om
 * det samme.
 */
async function handleWarning(data: unknown): Promise<void> {
  const code =
    data && typeof data === "object"
      ? ((data as Partial<BackendWarning>).code ?? "")
      : "";

  // Denne er synkron og skjer uansett hva planen sier: «Lytter»-brikka står
  // over en død buffer helt til noe sier fra. Se `state/preroll.ts` — bakenden
  // har gitt opp etter tre forsøk, så troen om at løkka går er en løgn, og
  // brikka er den eneste kvitteringen på at lyden fra før knappetrykket blir
  // tatt vare på.
  if (code === "preroll_dead") markPrerollDead();

  if (code === "disk_low") await refreshDiskSpace();
  if (code === "device_missing") await loadAudioDevices();

  const plan = planWarning(data, currentSurfaces());
  if (plan.action === "raise") {
    raiseBanner(plan.banner);
    return;
  }
  if (plan.action === "deduped") {
    // Den avledede flaten har tatt over saken. Sto det et bakende-banner om
    // den fra en tidligere runde, skal det bort — ellers er duplikatet nettopp
    // det vi kom for å unngå, bare forskjøvet i tid.
    const key = WARNING_BANNER_KEYS[plan.code];
    if (key) dismissBanner(key);
  }
}

/**
 * Abonner på kanalen. Idempotent — et andre kall gir den samme opprydderen i
 * stedet for et andre sett lyttere på den samme kanalen.
 */
export function initBackendWarnings(): () => void {
  if (dispose) return dispose;

  const off = window.api.on("backend-warning", (data: unknown) => {
    void handleWarning(data);
  });

  dispose = () => {
    off?.();
    dispose = null;
  };
  return dispose;
}
