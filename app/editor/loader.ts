/**
 * Å åpne et opptak — fire ting, i denne rekkefølgen, og `loadSeq` over dem alle.
 *
 *   1. **Varigheten**, fra `editor_load_recording` (ffprobe leser bare
 *      container-hodet), så tidslinja finnes på millisekunder selv for en
 *      gudstjeneste på flere gigabyte.
 *   2. **Transporten**: `<audio>`-elementet peker på ORIGINALEN over `asset://`.
 *      En omkodet mellomfil er reserveløsningen, og den tas bare når webviewen
 *      ikke har en dekoder for containeren — eller når originalen viser seg
 *      ikke å åpne likevel.
 *   3. **Bølgeformen**: bakenden strømmer dekodingen rett inn i 100 topper i
 *      sekundet og cacher svaret ved siden av opptaket. Ingen lydbuffer bygges
 *      i renderer-en, så en FLAC på fire timer koster like lite minne som en på
 *      fire minutter.
 *   4. **Sidevognene**: kutt-utkastet fra en økt som ble avbrutt.
 *
 * `E.loadSeq` vokter alt sammen: hver `await` sjekker den på nytt, så en
 * bruker som åpner fil nummer to midt i lastingen aldri får den førstes
 * varighet, topper eller transport.
 *
 * ## Fremdriften sier hvilken fase den er i
 *
 * «Analyserer …» alene er ikke sant nok: en førstegangsåpning av en to timers
 * gudstjeneste bruker mesteparten av tiden på bølgeformen, og en fil webviewen
 * ikke kan spille bruker den på omkodingen. Begge har hver sin tekst, og begge
 * har en EKTE bar — bakenden rapporterer sin dekodingsposisjon på
 * `editor-peaks-progress` og `editor-proxy-progress`.
 *
 * Tekstene er legacys egne (`editor.analyzingWaveform`,
 * `editor.preparingPlayback`) og finnes derfor allerede i alle sju språk.
 */

import { routePlayback } from "@lib/pages/editor/play-regions";

import { cancelDraftSave, resetHistoryMirror, restoreDraftCuts } from "./cuts";
import {
  analyzing,
  E,
  fileName,
  filePath,
  loadError,
  loadPhase,
  loadProgress,
  loadState,
  mediaInfo,
  resetFileState,
  startedAtMs,
  duration as durationSignal,
} from "./model";
import { resetExport } from "./export";
import { resetSound } from "./sound";
import {
  ensurePlayerEl,
  seekTo,
  setPlaybackSource,
  teardownPlayback,
} from "./playback";
import { runAnalysis } from "./sermon";
import { fitAll } from "./viewport";
import { scheduleDraw } from "./waveform";

/** Hvor lenge et element får på seg å melde at det kan spille. Romslig nok for
 *  en kald ekstern disk, kort nok til at brukeren ikke sitter med en
 *  avspillingsknapp som ikke gjør noe. Legacys eget tall. */
const PLAYER_READY_TIMEOUT_MS = 5000;

/** Utkast eldre enn dette legges ikke tilbake. Legacys egen grense: å bli møtt
 *  av kutt man gjorde for to måneder siden er ikke gjenoppretting. */
const DRAFT_MAX_AGE_MS = 7 * 86_400_000;

/**
 * Containere webviewen dekoder selv.
 *
 * Lydsiden er `@lib/pages/editor/play-regions`' egen `routePlayback` —
 * PLAYER_EXTS, uendret. Videocontainerne er lagt til her og ikke der, fordi de
 * betyr noe ANNET i legacy: der peker de på `<video>`-elementet, som P4a ikke
 * har. Her spilles lydsporet av gjennom det samme `<audio>`-elementet som alt
 * annet, og både WKWebView (CoreMedia) og Chromium gjør det uten videre.
 *
 * ⚠️ Bildet vises fortsatt ikke mens man klipper. Klippingen er den samme —
 * tidslinja, kuttene og eksporten er lydens uansett — men den som redigerer en
 * videofil ser bare bølgeformen. P4b bygde den andre halvdelen av spørsmålet:
 * «Ta med video (MP4)» i steg 3 BEVARER bildet i eksporten (`hasVideo`
 * nedenfor er det som lar bryteren finnes). Å VISE det mens man klipper er
 * fortsatt en eierbeslutning.
 */
const VIDEO_ELEMENT_EXTS = new Set([".mp4", ".m4v", ".mov"]);

function routeForEditor(ext: string): "element" | "proxy" {
  const norm = ext.trim().toLowerCase();
  const dotted = norm.startsWith(".") ? norm : `.${norm}`;
  if (VIDEO_ELEMENT_EXTS.has(dotted)) return "element";
  return routePlayback(dotted);
}

/** Det biblioteket vet om raden, og som editoren ikke kan lese ut av fila. */
export interface OpenContext {
  /** Millisekundet gudstjenesten begynte — radens tittel i Bibliotek. */
  startedAtMs?: number | null;
  /**
   * Sekundet spillehodet skal stå på når fila er ferdig lastet.
   *
   * `window.openEditorWithFile(path, seekToSec)` har alltid tatt imot det, og
   * kontrakten er derfor ikke ny. Det brukes ETTER lastingen, aldri under:
   * legacy hadde en `CustomEvent`-vei som var kappløpsutsatt fordi `loadFile`
   * nullstiller posisjonen underveis.
   */
  seekToSec?: number | null;
}

/**
 * Lukk den åpne fila.
 *
 * Bekreftelsen ved ulagrede endringer hører til FLATEN, ikke hit: den er en
 * setning en frivillig leser, og modellen har ingen katalog.
 */
export function closeFile(): void {
  cancelDraftSave();
  teardownPlayback();
  resetSound();
  resetExport();
  resetFileState();
  resetHistoryMirror();
  loadState.value = "idle";
}

/** Den native åpne-dialogen. `null` = avbrutt, og da skjer ingenting. */
export async function pickAndOpen(): Promise<void> {
  const picked = await window.api.editorPickFile();
  if (picked) void openFile(picked);
}

export async function openFile(
  path: string,
  context: OpenContext = {},
): Promise<void> {
  const seq = ++E.loadSeq;
  // FØRST, før et eneste felt nullstilles: slå av forrige fils ventende
  // utkast-skriving. Den er debouncet i to sekunder, så et raskt filbytte lot
  // en timer stå armet som fyrte midt i lastingen — i verste fall inn i den
  // NYE filas sidevogn, rett før dens egen gjenoppretting leste den.
  cancelDraftSave();
  teardownPlayback();
  resetSound();
  resetExport();
  resetFileState();
  resetHistoryMirror();

  E.filePath = path;
  E.fileName = basename(path);
  E.startedAtMs = context.startedAtMs ?? null;
  filePath.value = E.filePath;
  fileName.value = E.fileName;
  startedAtMs.value = E.startedAtMs;
  loadState.value = "loading";
  loadPhase.value = null;
  loadProgress.value = null;
  loadError.value = null;

  const ext = extensionOf(path);

  // 1. Varigheten — og de tre andre tingene den samme probingen allerede vet.
  //
  // `hasVideo` styrer «Ta med video (MP4)» i steg 3, og `channels`/`sampleRate`
  // er det størrelsesanslaget regner med. Alle tre kom gratis med denne
  // ffprobe-en fra før; et eget `editor_probe_streams` ville vært en ny
  // prosess for et svar vi allerede holder i hånda.
  let seconds = 0;
  try {
    const info = await window.api.editorLoadRecording(path);
    if (info && Number.isFinite(info.durationSec) && info.durationSec > 0) {
      seconds = info.durationSec;
    }
    if (info) {
      mediaInfo.value = {
        hasVideo: info.hasVideo === true,
        channels: info.channels ?? null,
        sampleRate: info.sampleRate ?? null,
      };
    }
  } catch {
    seconds = 0;
  }
  if (seq !== E.loadSeq) return;

  // 2. Transporten.
  const el = ensurePlayerEl();
  let watching: Promise<boolean> | null = null;
  if (routeForEditor(ext) === "proxy") {
    await attachProxy(path, seq);
    if (seq !== E.loadSeq) return;
  } else {
    await window.api.editorAllowAssetPath(path);
    if (seq !== E.loadSeq) return;
    setPlaybackSource("original");
    el.src = window.api.toAssetUrl(path);
    el.load();
    // Bevisst IKKE ventet på: arbeidsflaten males så snart bølgeformen er
    // inne. Viser det seg at originalen ikke lar seg åpne, byttes en
    // mellomfil inn under den.
    watching = whenPlayable(el, PLAYER_READY_TIMEOUT_MS);
  }

  // 3. Bølgeformen.
  const peaks = await withPhase(
    "analyzingWaveform",
    "editor-peaks-progress",
    window.api.editorExtractAudioPeaks(path),
  );
  if (seq !== E.loadSeq) return;
  if (peaks && Array.isArray(peaks.peaks) && peaks.peaks.length > 0) {
    E.peaks = Float32Array.from(peaks.peaks);
    if (seconds <= 0) seconds = E.peaks.length / 100;
  }

  if (seconds <= 0) {
    // Verken ffprobe eller uttrekket visste: spør elementet selv. Bare
    // nåbart når ffmpeg mangler helt.
    const ok = watching
      ? await watching
      : await whenPlayable(el, PLAYER_READY_TIMEOUT_MS);
    if (seq !== E.loadSeq) return;
    if (ok && Number.isFinite(el.duration) && el.duration > 0) {
      seconds = el.duration;
    }
    watching = null;
  }

  if (seconds <= 0) {
    loadState.value = "error";
    loadError.value = "unreadable";
    return;
  }

  E.duration = seconds;
  durationSignal.value = seconds;
  if (!E.peaks) {
    // Flat bølgeform framfor en tom skjerm: kutt og eksport arbeider på
    // tidslinja, og den har vi nå.
    E.peaks = new Float32Array(Math.ceil(seconds * 100));
  }
  fitAll();

  // 4. Sidevogna. Et utkast som finnes betyr at forrige økt endte midt i noe.
  try {
    const draft = (await window.api.editorReadCutsDraft(path)) as {
      cuts?: Array<{ start: number; end: number }>;
      ts?: number;
    } | null;
    if (seq === E.loadSeq && draft && Array.isArray(draft.cuts)) {
      const age = draft.ts ? Date.now() - draft.ts : 0;
      const fresh = !draft.ts || age < DRAFT_MAX_AGE_MS;
      const valid = draft.cuts.filter(
        (c) =>
          typeof c.start === "number" &&
          typeof c.end === "number" &&
          c.end > c.start,
      );
      if (fresh && valid.length > 0) restoreDraftCuts(valid);
    }
  } catch {
    /* et utkast som ikke lot seg lese er ikke en grunn til å ikke åpne fila */
  }
  if (seq !== E.loadSeq) return;

  loadPhase.value = null;
  loadProgress.value = null;
  loadState.value = "ready";
  if (typeof context.seekToSec === "number") seekTo(context.seekToSec);
  scheduleDraw();

  if (watching) void watchOriginal(watching, path, seq);

  // Analysen kjører alltid ved åpning, men etter første maling: den er nok en
  // full ffmpeg-passering over opptaket, og å starte den før arbeidsflaten er
  // på skjermen ville latt to dekodinger av den samme filen konkurrere.
  //
  // Flagget settes HER og ikke inne i `runAnalysis`: mellom «klar» og «analysen
  // begynner» er det et opphold, og i det oppholdet ville skjermen sagt «vi
  // fant ingen preken» — som er usant, for vi har ikke lett ennå.
  analyzing.value = true;
  whenIdle(() => {
    if (seq === E.loadSeq) void runAnalysis(seq);
  });
}

// ── Transporten ─────────────────────────────────────────────────────────────

/**
 * Bytt inn mellomfila hvis originalen aldri ble spillbar.
 *
 * Grunnene finnes: et profil-valg webviewen ikke har dekoder for, et skadet
 * filhode, et nettverksvolum som stopper opp. Uten dette ville
 * avspillingsknappen stått der og ikke gjort noe.
 */
async function watchOriginal(
  ready: Promise<boolean>,
  path: string,
  seq: number,
): Promise<void> {
  const ok = await ready;
  if (seq !== E.loadSeq || ok || E.playbackSource !== "original") return;
  await attachProxy(path, seq);
}

/**
 * Pek elementet på en omkodet AAC-mellomfil.
 *
 * SISTE utvei: enten har containeren ingen dekoder i webviewen i det hele
 * tatt, eller originalen nektet å åpne. Den koster en full omkoding (et minutt
 * eller mer på en gudstjeneste), så den tas aldri på spekulasjon.
 */
async function attachProxy(path: string, seq: number): Promise<boolean> {
  let proxy: string | null;
  try {
    proxy = await withPhase(
      "preparingPlayback",
      "editor-proxy-progress",
      window.api.editorExtractPlaybackProxy(path),
    );
  } catch {
    proxy = null;
  }
  if (seq !== E.loadSeq) return false;
  if (!proxy) {
    // Klipping og eksport går på originalen uansett, så dette er en beskjed,
    // ikke en feiltilstand.
    setPlaybackSource("none");
    return false;
  }

  await window.api.editorAllowAssetPath(proxy);
  if (seq !== E.loadSeq) return false;
  const el = ensurePlayerEl();
  el.src = window.api.toAssetUrl(proxy);
  el.load();
  setPlaybackSource("proxy");
  void whenPlayable(el, PLAYER_READY_TIMEOUT_MS).then((ok) => {
    // Snakk bare for den transporten vi faktisk hektet på.
    if (seq === E.loadSeq && E.playbackSource === "proxy" && !ok) {
      setPlaybackSource("none");
    }
  });
  return true;
}

/**
 * Løser når elementet kan si hvor det er, eller `false` når det feiler eller
 * aldri kommer dit.
 *
 * ÉN opprydding for timeren og begge lytterne: en foreldreløs timer pluss
 * gamle lyttere på et GJENBRUKT element er nøyaktig slik forrige fils lasting
 * korrumperte den neste da brukeren byttet fort.
 */
function whenPlayable(
  el: HTMLMediaElement,
  timeoutMs: number,
): Promise<boolean> {
  if (el.readyState >= 1) return Promise.resolve(true);
  return new Promise<boolean>((resolve) => {
    let timer = 0;
    const cleanup = (): void => {
      clearTimeout(timer);
      el.removeEventListener("loadedmetadata", onMeta);
      el.removeEventListener("error", onErr);
    };
    const onMeta = (): void => {
      cleanup();
      resolve(true);
    };
    const onErr = (): void => {
      cleanup();
      resolve(false);
    };
    el.addEventListener("loadedmetadata", onMeta, { once: true });
    el.addEventListener("error", onErr, { once: true });
    timer = window.setTimeout(() => {
      cleanup();
      resolve(false);
    }, timeoutMs);
  });
}

// ── Fremdriften ─────────────────────────────────────────────────────────────

/**
 * Kjør `work` mens lastingen sier hvilken fase den er i, og tegn baren fra
 * bakendens egne tikk.
 *
 * Abonnementet starter MED ÉN GANG selv om teksten kan rekke å bli byttet ut:
 * bakendens første tikk slår lett 400 ms, og en bar som starter på null når
 * den egentlig er halvveis er en bar som lyver den ene gangen den betyr noe.
 */
async function withPhase<T>(
  phase: string,
  channel: string,
  work: Promise<T>,
): Promise<T> {
  loadPhase.value = phase;
  loadProgress.value = null;
  // `on` er valgfri i shimmens type (den finnes ikke i hvert vertsmiljø), og
  // et abonnement som ikke lot seg tegne betyr bare en ubestemt bar.
  const unsub = window.api.on?.(channel, (payload: unknown) => {
    const fraction = (payload as { fraction?: number } | null)?.fraction;
    if (typeof fraction !== "number" || !Number.isFinite(fraction)) return;
    loadProgress.value = Math.max(0, Math.min(1, fraction));
  });
  try {
    return await work;
  } finally {
    unsub?.();
    loadProgress.value = null;
  }
}

/**
 * Kjør `fn` når hovedtråden neste gang er ledig, så den ikke konkurrerer med
 * arbeidsflatens første maling. Faller tilbake på en vanlig timer der
 * `requestIdleCallback` mangler (Safari/WKWebView, altså hver macOS-bygging) —
 * fristen er den samme uansett; dette er en utsettelse, ikke en scheduler.
 */
function whenIdle(fn: () => void): void {
  const ric = (
    window as unknown as {
      requestIdleCallback?: (
        cb: () => void,
        opts?: { timeout: number },
      ) => number;
    }
  ).requestIdleCallback;
  if (typeof ric === "function") ric(fn, { timeout: 1500 });
  else window.setTimeout(fn, 1500);
}

// ── Små ting ────────────────────────────────────────────────────────────────

function basename(path: string): string {
  return path.split(/[/\\]/).pop() ?? path;
}

function extensionOf(path: string): string {
  const name = basename(path);
  const at = name.lastIndexOf(".");
  return at > 0 ? name.slice(at).toLowerCase() : "";
}
