/**
 * Avspillingen i Klipp-steget — en fjernkontroll for avspillingsmotoren i Rust.
 *
 * ## Det er ikke lenger et `<audio>`-element (B5c)
 *
 * Før spilte webviewen opptaket gjennom et medieelement på `asset://`, hoppet over kuttene med
 * `currentTime` og tonet `volume` ned og opp rundt hvert hopp for å skjule klikket, og leste
 * spillehodet av `currentTime`. Nå spiller Rust (`src-tauri/src/playback/`, B5a): ffmpeg
 * dekoder opptaket, kutt-planen legger programmet ende mot ende med 20 ms konstant-effekt-
 * overganger, og lydkortet er klokka. Denne fila sender bare meldinger (spill, pause, hopp,
 * nye kutt) og lar spillehodet følge det motoren melder. Det finnes ingen volumrampe her, ingen
 * hoppløkke og ingen mellomfil: kuttene er en del av programmet motoren spiller.
 *
 * Økta er den samme som Lyd-steget bruker (`sound.ts`, `openEngine`): én motor, ett lydkort.
 * Den åpnes av første «spill» (eller første hopp mens den er åpen) og lukkes når steget
 * forlates eller fila byttes. Klipp-steget ber ikke om noen kjede: det lytter til opptaket
 * slik det er, med kuttene.
 *
 * ## Forhåndslytting er den eneste modusen
 *
 * Én knapp, og den hopper alltid over kuttene: en frivillig som nettopp har trykket «Behold
 * bare prekenen» vil høre RESULTATET.
 *
 * ## Spillehodet er det motoren sier
 *
 * Motoren melder posisjonen (kildesekunder) ~30 ganger i sekundet, regnet av frames lydkortet
 * faktisk har tatt. `E.playStartSec` skrives herfra, og bare derfra mens det spiller. Tegningen
 * skjer i `waveform.ts` og leser `E` direkte; det ENE signalet som skrives er `playheadSec`, og
 * det skjer på husets tegne-kadens (`@lib/ui/frame-gate`) — tallet «0:21:08» endrer seg ett
 * sekund om gangen.
 *
 * ## Nye kutt mens det spiller
 *
 * Kuttlista er en del av programmet. Når den endres (et nytt kutt, angre, en dratt grense), får
 * motoren den nye planen og fortsetter fra samme sted i kilden som et hopp: nedtoning, nytt
 * program, opptoning. Et kutt midt i avspilling koster altså et lite opphold, ikke et klikk.
 * Endringene samles (`CUTS_DEBOUNCE_MS`) så en rekke angre-trykk ikke starter motoren på nytt
 * for hvert.
 *
 * ## Video
 *
 * Klipp-steget viser ikke bildet (se `loader.ts`); det finnes derfor ingen `<video>` som må
 * følge motoren. Lyden i en videofil spilles av motoren som alt annet. Den dagen bildet vises,
 * er det en dempet `<video>` som følger posisjonen herfra og rettes når den driver mer enn ett
 * bilde.
 */

import { DRAW_INTERVAL_MS, nextDrawGate } from "@lib/ui/frame-gate";
import type { EditorListenEvent } from "@legacy/bindings/EditorListenEvent";
import { effect } from "@preact/signals";

import { clampToFile } from "./geometry";
import {
  cuts,
  E,
  playbackSource,
  playheadSec,
  playing,
  type PlaybackSource,
} from "./model";
import { engineIsOpen, openEngine, stopListen, subscribeEngine } from "./sound";
import { followPlayhead } from "./viewport";
import { scheduleDraw } from "./waveform";

/** Hvor lenge nye kutt samles før motoren får dem. */
export const CUTS_DEBOUNCE_MS = 120;

/** Under dette fra slutten regnes fila som spilt ut, og neste «spill» begynner på nytt. */
const AT_END_SEC = 0.05;

/** Der programmet slutter i kilden: filas slutt, eller starten på et kutt som går ut fila. */
function playableEnd(): number {
  let end = E.duration;
  for (const c of E.cuts) {
    if (c.end >= E.duration - 0.001) end = Math.min(end, c.start);
  }
  return end;
}

/** Hvert start/stopp øker denne. Åpningen av lydkortet er asynkron, og en callback fra en
 *  AVLØST avspilling må ikke starte motoren oppå en ny. */
let playGen = 0;

/** Siste posisjon motoren meldte (eller ble bedt om), kildesekunder. */
let engineAt = 0;

/** Høyeste generasjon sett i en posisjon. Et «ferdig» fra en eldre generasjon (programmet ble
 *  byttet rett før slutten) er forsinket og betyr ikke at det nye er spilt ut. */
let maxGenSeen = 0;

let unsubscribe: (() => void) | null = null;
let stopCutWatch: (() => void) | null = null;
let cutsTimer: ReturnType<typeof setTimeout> | null = null;

/** Fortell modellen hvordan avspillingen faktisk går. */
export function setPlaybackSource(source: PlaybackSource): void {
  E.playbackSource = source;
  playbackSource.value = source;
}

/**
 * Slipp avspillingen: stopp, og lukk motoren så lydkortet frigis. Kalles når fila byttes eller
 * lukkes, og når Klipp-steget forlates.
 */
export function teardownPlayback(): void {
  stopPlay();
  stopCutWatch?.();
  stopCutWatch = null;
  cancelCutsTimer();
  stopListen();
  setPlaybackSource("original");
}

/** Kutt er hopp-soner. Et spillehode som hviler inne i ett er meningsløst —
 *  det spilles ingen lyd der. Legacys `snapOutOfCut`, ordrett. */
export function snapOutOfCut(sec: number): number {
  for (const cut of E.cuts) {
    if (sec >= cut.start && sec < cut.end) return Math.min(E.duration, cut.end);
  }
  return sec;
}

/** Flytt spillehodet dit, og stopp det som spiller. */
export function seekTo(sec: number): void {
  stopPlay();
  E.playStartSec = snapOutOfCut(clampToFile(sec));
  publishPlayhead(true);
  // Står motoren åpen, flytter vi den nå (pauset), så neste «spill» er hørt med en gang.
  if (engineIsOpen()) sendSeek(E.playStartSec);
  scheduleDraw();
}

/**
 * Flytt spillehodet `deltaSec` (piltastene). Som i et redigeringsprogram
 * fortsetter avspillingen fra det nye stedet hvis den gikk. Bakover inn i et
 * kutt lander FØR kuttet — `snapOutOfCut` alene ville kastet spillehodet
 * FORBI det, altså framover, på en tast som betyr «tilbake».
 */
export function seekBy(deltaSec: number): void {
  // `E.playStartSec`, ikke `engineAt`: hver posisjon fra motoren skriver den
  // (se `onEngineEvent`), og FØR første posisjon — mens lydkortet åpnes — er
  // den stedet avspillingen ble bedt om å starte. `engineAt` henger da igjen
  // på forrige økt, så → kort etter mellomrom hoppet fra feil sted (review).
  const from = E.playStartSec;
  let target = clampToFile(from + deltaSec);
  if (deltaSec < 0) {
    for (const cut of E.cuts) {
      if (target >= cut.start && target < cut.end) {
        target = Math.max(0, cut.start - 0.01);
      }
    }
  }
  seekAndResume(target);
}

/** Home/End: start eller slutt av programmet. */
export function seekToEdge(edge: "start" | "end"): void {
  seekAndResume(edge === "start" ? 0 : playableEnd());
}

function seekAndResume(sec: number): void {
  const wasPlaying = E.isPlaying;
  const target = snapOutOfCut(clampToFile(sec));
  // Mens det spiller og motoren er åpen, hoppes det UTEN å stoppe: en holdt ←/→ gir ~30
  // hopp i sekundet, og stopp→hopp→spill for hvert ga 30 pause/spill-par og en ny avspilling
  // (ny `playGen`) per tastetrykk. Motoren holder seg i spill gjennom et hopp. Et pre-hopp-
  // posisjonsrop kan nå skrive spillehodet tilbake én gang; neste melding (~33 ms) retter det.
  // Er motoren ikke åpen ennå (lydkortet åpnes), eller hoppet lander ved slutten, tas den
  // vanlige veien: stopp, flytt, og start på nytt.
  if (wasPlaying && engineIsOpen() && target < playableEnd() - AT_END_SEC) {
    E.playStartSec = target;
    publishPlayhead(true);
    sendSeek(target);
    scheduleDraw();
    return;
  }
  seekTo(sec);
  if (wasPlaying && E.playStartSec < playableEnd() - AT_END_SEC) startPlay();
}

export function togglePlay(): void {
  if (E.isPlaying) {
    stopPlay();
    return;
  }
  startPlay();
}

export function startPlay(): void {
  const gen = ++playGen;
  if (!E.sourceToken) return;
  maxGenSeen = 0;

  let from = snapOutOfCut(clampToFile(E.playStartSec));
  // Spilt ut: begynn på nytt, som et medieelement gjør.
  if (E.duration > 0 && from >= playableEnd() - AT_END_SEC) {
    from = snapOutOfCut(0);
  }
  E.playStartSec = from;
  E.isPlaying = true;
  playing.value = true;
  resetDrawGate();
  publishPlayhead(true);

  void (async () => {
    const wasOpen = engineIsOpen();
    const ok = await openEngine(from);
    // Stoppet, eller startet på nytt, mens lydkortet åpnet.
    if (gen !== playGen || !E.isPlaying) return;
    if (!ok) {
      console.warn("[editor] avspillingsmotoren kunne ikke åpnes");
      stopPlay();
      setPlaybackSource("none");
      return;
    }
    setPlaybackSource("original");
    watchEngine();
    watchCuts();
    flushCuts();
    if (wasOpen && Math.abs(engineAt - from) > 0.02) sendSeek(from);
    else engineAt = from;
    await window.api.editorListenTransport({ kind: "play" });
  })();
}

export function stopPlay(): void {
  playGen++;
  const wasPlaying = E.isPlaying;
  const at = wasPlaying ? engineAt : E.playStartSec;
  unsubscribe?.();
  unsubscribe = null;
  E.isPlaying = false;
  playing.value = false;
  if (wasPlaying) {
    E.playStartSec = clampToFile(at);
    // Motoren toner ut på noen millisekunder; sekvensen er fortsatt åpen for neste «spill».
    void window.api.editorListenTransport({ kind: "pause" });
  }
  publishPlayhead(true);
  scheduleDraw();
}

// ── Motorens hendelser ──────────────────────────────────────────────────────

let drawGate = 0;

function resetDrawGate(): void {
  drawGate = performance.now() - DRAW_INTERVAL_MS;
}

/**
 * Skriv spillehodet til speilet — men bare når gaten slipper, eller når
 * `force` sier at dette er en enkelthendelse (et søk, en stopp) som skal vises
 * med én gang.
 */
function publishPlayhead(force: boolean): void {
  if (!force) {
    const next = nextDrawGate(drawGate, performance.now());
    if (next === drawGate) return;
    drawGate = next;
  }
  playheadSec.value = E.playStartSec;
}

function watchEngine(): void {
  if (unsubscribe) return;
  unsubscribe = subscribeEngine(onEngineEvent);
}

function onEngineEvent(e: EditorListenEvent): void {
  if (!E.isPlaying) return;
  switch (e.kind) {
    case "stopped":
      // Et opptak startet og Rust lukket økta: spillehodet blir stående der motoren var.
      stopPlay();
      return;
    case "position": {
      maxGenSeen = Math.max(maxGenSeen, e.gen);
      const at = clampToFile(e.sourceSecs);
      engineAt = at;
      E.playStartSec = at;
      publishPlayhead(false);
      followPlayhead(at);
      scheduleDraw();
      return;
    }
    case "ended":
      if (e.gen < maxGenSeen) return;
      // Programmet er spilt ut: spillehodet står ved slutten av det som ble spilt.
      engineAt = playableEnd();
      stopPlay();
      return;
    case "error":
      console.warn("[editor] avspillingsmotoren meldte feil", e.message);
      stopPlay();
      setPlaybackSource("none");
      return;
    default:
      return;
  }
}

/** Be motoren hoppe (kildesekunder). Innenfor et kutt: neste del som blir igjen. */
function sendSeek(sec: number): void {
  engineAt = sec;
  void window.api.editorListenTransport({ kind: "seekSource", secs: sec });
}

// ── Kuttene er en del av programmet ─────────────────────────────────────────

function cancelCutsTimer(): void {
  if (cutsTimer === null) return;
  clearTimeout(cutsTimer);
  cutsTimer = null;
}

/** Følg kuttlista så lenge motoren er åpen. Første kjøring er økta som nettopp ble åpnet med
 *  de samme kuttene, og sender ingenting. */
function watchCuts(): void {
  if (stopCutWatch) return;
  let first = true;
  stopCutWatch = effect(() => {
    void cuts.value;
    if (first) {
      first = false;
      return;
    }
    cancelCutsTimer();
    cutsTimer = setTimeout(flushCuts, CUTS_DEBOUNCE_MS);
  });
}

/** Send kuttlista til motoren nå, hvis den er åpen og noe venter. */
function flushCuts(): void {
  const pending = cutsTimer !== null;
  cancelCutsTimer();
  if (!pending || !engineIsOpen()) return;
  const kept = E.duration - E.cuts.reduce((s, c) => s + (c.end - c.start), 0);
  if (E.isPlaying && kept <= 0.001) {
    // Alt er klippet bort: det er ingenting å spille.
    stopPlay();
    return;
  }
  const at = E.isPlaying ? engineAt : snapOutOfCut(E.playStartSec);
  if (!E.isPlaying && at !== E.playStartSec) {
    E.playStartSec = at;
    publishPlayhead(true);
    scheduleDraw();
  }
  engineAt = at;
  void window.api.editorListenTransport({
    kind: "cuts",
    cutRegions: E.cuts.map((c) => ({ start: c.start, end: c.end })),
    fullLengthSec: E.duration,
    atSecs: at,
  });
}
