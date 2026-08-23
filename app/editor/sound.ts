/**
 * Steg 2 — LYD. Tilstanden, analysen og de tjue sekundene man lytter på.
 *
 * Tabellen som gjør de tre ordene om til tall bor i `sound-profiles.ts` og er
 * ren. Her er alt som har en bivirkning: et signal per ting treet viser, ÉN
 * memoisert kanalanalyse per fil, og et eget `<audio>`-element for lyttingen.
 *
 * ## Hvorfor lyttingen har sitt EGET element
 *
 * Transporten i steg 1 eier `E.playerEl` og skriver `playheadSec` seksti ganger
 * i sekundet mens den går. Å låne det elementet til en 20-sekunders
 * forhåndslytting ville flyttet spillehodet i bølgeformen, og etterpå stått
 * igjen med `src` pekende på en temp-fil som slettes ved neste oppstart. To
 * jobber, to elementer — og begge eies av en modul, ikke av JSX, av samme grunn
 * som `E.playerEl`: et element Preact rendrer mister `src`-en sin når noe over
 * det tegnes på nytt.
 *
 * ## «Etter» er en EKTE gjengivelse, ikke en simulering
 *
 * `editor_master_preview` kjører profilens filterkjede + loudnorm over et
 * klemt utsnitt og skriver en mp3 i `std::env::temp_dir()` — ALDRI ved siden av
 * originalen. `_mastert`-fila fra atlasets sted 2 finnes ikke i dette skallet,
 * og den er heller ikke det som lages her.
 *
 * ⚠️ Utsnittet blir liggende i OS-ens temp-mappe til OS-en selv rydder den.
 * Kjernen HAR predikatet som kjenner igjen dem (`is_preview_temp_name`,
 * `sundayrec-master-preview-*.mp3`), men ingen sveip i `src-tauri` kaller det —
 * en gjeld fra mastring-panelet, ikke en P4b innfører. Hver fil er ~800 kB
 * (20 s, 320 kbps), én per profil per opptak. Det ryddes ikke herfra: å kalle
 * `editor_cleanup_temp_files` ville sett riktig ut og gjort noe helt annet —
 * den sveipen sletter `.__editor_tmp`/`.__editor_bak` ved siden av OPPTAKET.
 */

import { signal } from "@preact/signals";
import type { ExportChannelRepair } from "@lib/pages/editor/export-params";
import { defaultProcessing, type Processing } from "@lib/pages/editor/mixer";

import { E } from "./model";
import { sermonWindow } from "./editor-core";
import {
  DEFAULT_SOUND_PROFILE,
  LISTEN_SPAN_SEC,
  listenStartSec,
  specFor,
  type SoundProfile,
} from "./sound-profiles";

// ── Valget ──────────────────────────────────────────────────────────────────

/** Profilen som gjelder. `none` er også det bryteren AV betyr — ÉN sannhet, to
 *  måter å uttrykke den på i UI-et, slik canvasen tegnet det. */
export const soundProfile = signal<SoundProfile>(DEFAULT_SOUND_PROFILE);

/** Profilen bryteren skal slå TILBAKE til. Uten den ville «av, så på» kastet
 *  et valg brukeren tok: en som står på «Tale og musikk» og slår av og på
 *  igjen skal ikke havne på «Tale». */
const lastActiveProfile = signal<SoundProfile>(DEFAULT_SOUND_PROFILE);

/** Har brukeren VÆRT på steget? Haken i stegstripa venter på det: «Tale» er
 *  standarden, og en hake fra første sekund ville påstått at noen bestemte
 *  seg. */
export const soundVisited = signal(false);

export function setSoundProfile(next: SoundProfile): void {
  soundProfile.value = next;
  if (next !== "none") lastActiveProfile.value = next;
}

/** Bryteren «Automatisk lydforbedring». AV = «Ingen». */
export function setAutoEnhance(on: boolean): void {
  soundProfile.value = on ? lastActiveProfile.value : "none";
}

// ── Den avanserte mikseren ──────────────────────────────────────────────────

/** Er mikseren i bruk? Da ERSTATTER den profilen — se `soundExportFields`. */
export const useMixer = signal(false);
/** Er mikserpanelet åpent? Å lukke panelet slår ikke av mikseren. */
export const mixerOpen = signal(false);

/**
 * Mikserens tilstand.
 *
 * Startverdiene er `defaultProcessing()` fra legacys mikser — det ENE stedet i
 * repoet der `VocalChain::default()` er speilet i TypeScript. Å skrive et andre
 * speil her ville vært et tredje sted de samme tallene kan bli uenige.
 */
export const mixer = signal<Processing>(defaultProcessing());

export function patchMixer(patch: Partial<Processing>): void {
  mixer.value = { ...mixer.value, ...patch };
}

/** Sett forsterkningen på ett EQ-bånd. Bånd med 0 dB fjernes ved sending. */
export function setEqBand(freqHz: number, gainDb: number, q: number): void {
  const rest = mixer.value.eq.filter((b) => b.freqHz !== freqHz);
  mixer.value = {
    ...mixer.value,
    eq: [...rest, { freqHz, gainDb, q }].sort((a, b) => a.freqHz - b.freqHz),
  };
}

/** Nyttelastens `processing`: mikseren minus båndene som ikke gjør noe. Et
 *  equalizer-filter på 0 dB er et filter som koster tid og ikke endrer lyd. */
export function mixerProcessing(): Processing {
  const p = mixer.value;
  return { ...p, eq: p.eq.filter((b) => Math.abs(b.gainDb) > 0.01) };
}

// ── Kanalanalysen ───────────────────────────────────────────────────────────

/** Reparasjonen `editor_auto_process` anbefalte, eller `null`. */
export const channelRepair = signal<ExportChannelRepair | null>(null);
/** Diagnosens kode (`balanced`, `dead_left`, …), for den ene ærlige linja. */
export const channelCode = signal<string | null>(null);
export const analyzingSound = signal(false);

let analysisFor = "";
let analysis: Promise<void> | null = null;

/**
 * Kjør kanalanalysen ÉN gang per fil, og la alle vente på den samme.
 *
 * Den koster en full `astats`-passering over opptaket, så den startes når
 * lyd-steget åpnes (ikke ved filåpning, der bølgeform-uttrekket og
 * segmentanalysen allerede konkurrerer om den samme fila) — og eksporten
 * venter på den, slik at en frivillig som går rett til steg 3 får den samme
 * reparasjonen som en som stoppet innom.
 *
 * En feilet analyse er ikke en feiltilstand: da er det ingen reparasjon å
 * anbefale, og eksporten går uten. Bakenden svarer `null` i en bygging uten
 * `editor`-featuren, og det er nøyaktig det samme svaret.
 */
export function ensureSoundAnalysis(): Promise<void> {
  const path = E.filePath;
  if (!path) return Promise.resolve();
  if (analysisFor === path && analysis) return analysis;
  analysisFor = path;
  analyzingSound.value = true;
  analysis = (async () => {
    try {
      const res = await window.api.editorAutoProcess(path);
      // En annen fil rakk å bli åpnet mens vi analyserte: svaret gjelder
      // ikke lenger noe som står på skjermen.
      if (E.filePath !== path) return;
      const rec = res?.diagnosis?.recommended;
      channelCode.value = res?.diagnosis?.code ?? null;
      channelRepair.value =
        rec && rec.mode && rec.mode !== "none"
          ? { mode: rec.mode, leftDb: rec.leftDb, rightDb: rec.rightDb }
          : null;
    } catch {
      /* ingen anbefaling er et gyldig svar */
    } finally {
      if (E.filePath === path) analyzingSound.value = false;
    }
  })();
  return analysis;
}

// ── Før/etter-lyttingen ─────────────────────────────────────────────────────

export type ListenSide = "before" | "after";

export const listenSide = signal<ListenSide>("after");
export const listenPlaying = signal(false);
export const listenBusy = signal(false);
export const listenError = signal(false);

/** Utsnittets startsekund, for linja «0:21:08 — 20 sekunder fra prekenen». */
export const listenStart = signal(0);

/** Ferdige «Etter»-utsnitt, nøklet på preset-id. Å be bakenden gjengi de
 *  samme tjue sekundene på nytt hver gang man bytter fane er en ffmpeg-kjøring
 *  for et svar vi allerede har. */
const previews = new Map<string, string>();

let el: HTMLAudioElement | null = null;
let stopTimer = 0;

function player(): HTMLAudioElement {
  if (!el) {
    el = new Audio();
    el.preload = "metadata";
    el.addEventListener("ended", () => (listenPlaying.value = false));
    el.addEventListener("pause", () => (listenPlaying.value = false));
  }
  return el;
}

/** Regn ut hvor utsnittet begynner, ut fra prekenvinduet slik det står NÅ. */
export function refreshListenStart(): void {
  const window = sermonWindow({
    cuts: E.cuts,
    duration: E.duration,
    suggestion: E.suggestion,
    applied: E.applied,
  });
  listenStart.value = listenStartSec(window, E.duration);
}

/** Stopp lyttingen. Kalles når steget forlates og når fila lukkes. */
export function stopListen(): void {
  window.clearTimeout(stopTimer);
  stopTimer = 0;
  if (el) {
    el.pause();
    // `removeAttribute` og ikke `src = ''`: en tom streng er en RELATIV url og
    // får webviewen til å laste sida selv som media, med en feil i konsollen
    // for hver gang.
    el.removeAttribute("src");
    el.load();
  }
  listenPlaying.value = false;
}

export function setListenSide(side: ListenSide): void {
  if (listenSide.value === side) return;
  stopListen();
  listenSide.value = side;
  listenError.value = false;
}

/**
 * Spill de tjue sekundene, på den siden som er valgt.
 *
 * «Før» er ORIGINALEN fra utsnittets startsekund, stoppet av en timer — det
 * finnes ingen «spill fra … til …» på et medieelement. «Etter» er bakendens
 * gjengivelse, som allerede ER tjue sekunder lang og derfor stopper selv.
 */
export async function toggleListen(): Promise<void> {
  if (listenPlaying.value) {
    stopListen();
    return;
  }
  const spec = specFor(soundProfile.value);
  const side = listenSide.value;
  const start = listenStart.value;
  listenError.value = false;

  if (side === "after") {
    const preset = spec.masterPreset;
    if (!preset) return;
    const ready = previews.get(preset);
    const path = ready ?? (await renderPreview(preset, start));
    if (!path) {
      listenError.value = true;
      return;
    }
    await playFile(path, 0, null);
    return;
  }

  // «Før» — originalen. Den samme transporten steg 1 bruker, og den samme
  // ærlige begrensningen: uten en spillbar kilde er det ingenting å høre.
  if (E.playbackSource === "none") {
    listenError.value = true;
    return;
  }
  await playFile(E.filePath, start, LISTEN_SPAN_SEC);
}

async function renderPreview(
  preset: string,
  start: number,
): Promise<string | null> {
  listenBusy.value = true;
  try {
    const res = await window.api.masterPreview(
      E.filePath,
      preset,
      start,
      LISTEN_SPAN_SEC,
    );
    if (!res?.ok || !res.previewPath) return null;
    previews.set(preset, res.previewPath);
    return res.previewPath;
  } catch {
    return null;
  } finally {
    listenBusy.value = false;
  }
}

async function playFile(
  path: string,
  from: number,
  limitSec: number | null,
): Promise<void> {
  const audio = player();
  window.clearTimeout(stopTimer);
  try {
    await window.api.editorAllowAssetPath(path);
  } catch {
    /* de statiske globene dekker de vanlige mappene uansett */
  }
  audio.src = window.api.toAssetUrl(path);
  audio.load();
  if (from > 0) {
    // `currentTime` før metadataen er inne blir kastet av elementet, stille.
    await new Promise<void>((resolve) => {
      if (audio.readyState >= 1) return resolve();
      const done = (): void => resolve();
      audio.addEventListener("loadedmetadata", done, { once: true });
      audio.addEventListener("error", done, { once: true });
      window.setTimeout(done, 4000);
    });
    audio.currentTime = from;
  }
  try {
    await audio.play();
    listenPlaying.value = true;
  } catch {
    listenError.value = true;
    listenPlaying.value = false;
    return;
  }
  if (limitSec !== null) {
    stopTimer = window.setTimeout(() => stopListen(), limitSec * 1000);
  }
}

/** Glem alt som hørte til ÉN fil. */
export function resetSound(): void {
  stopListen();
  soundProfile.value = DEFAULT_SOUND_PROFILE;
  lastActiveProfile.value = DEFAULT_SOUND_PROFILE;
  soundVisited.value = false;
  useMixer.value = false;
  mixerOpen.value = false;
  mixer.value = defaultProcessing();
  channelRepair.value = null;
  channelCode.value = null;
  analyzingSound.value = false;
  listenSide.value = "after";
  listenError.value = false;
  listenBusy.value = false;
  listenStart.value = 0;
  analysisFor = "";
  analysis = null;
  previews.clear();
}
