/**
 * Avspillingen — ett `<audio>`-element, og forhåndslyttingen som hopper over
 * kuttene.
 *
 * ## Elementet eies av modellen
 *
 * `E.playerEl` er ETT element, laget første gang og gjenbrukt for hver fil.
 * Ikke JSX: et element i treet ville mistet `src`-en sin hver gang noe over
 * det rendret, og avspilling som stopper uten at noe feiler er den verste
 * formen for feil. Legacy gjør det samme, av samme grunn (`persistentPlayerEl`
 * — et nytt element per fil lekket en dekoder og et åpent filhåndtak hver
 * gang).
 *
 * ## Forhåndslytting er den eneste modusen
 *
 * Legacy har to knapper: «spill» (hele fila) og «forhåndslytt» (hopp over
 * kutt). En frivillig som nettopp har trykket «Behold bare prekenen» vil høre
 * RESULTATET; å måtte vite hvilken av to avspillingsknapper som viser det er
 * nøyaktig valget canvasens sett 4 fjerner. Så: én knapp, og den hopper alltid
 * over kuttene.
 *
 * ## De-klikket
 *
 * Et hardt `currentTime`-hopp over et kutt lander midt i bølgeformen på begge
 * sider, og steget i signalet er et hørbart klikk på hvert eneste kutt. Legacys
 * rampe er portet ordrett: tone ned over tre frames, hopp, tone opp igjen.
 * `volume` er den eneste knappen et medieelement gir (0..1, altså bare
 * demping), og det er akkurat det et de-klikk trenger.
 *
 * ## Spillehodet er frame-gatet
 *
 * Tegningen skjer i `waveform.ts` og leser `E` direkte. Det ENE signalet som
 * skrives fra løkka er `playheadSec`, og det skjer på husets tegne-kadens
 * (`@lib/ui/frame-gate`) — ikke per frame. Tallet «0:21:08» endrer seg ett
 * sekund om gangen, og seksti re-rendringer i sekundet for å vise det samme
 * tallet er den jank-en canvasen ellers ville fått skylda for.
 */

import { DRAW_INTERVAL_MS, nextDrawGate } from "@lib/ui/frame-gate";

import { clampToFile } from "./geometry";
import {
  E,
  playbackSource,
  playheadSec,
  playing,
  type PlaybackSource,
} from "./model";
import { followPlayhead } from "./viewport";
import { scheduleDraw } from "./waveform";

/** Hvor mange frames rampen bruker hver vei. Legacys eget tall. */
const DUCK_FRAMES = 3;

/**
 * Hvor lenge play venter på at elementet blir klart før det gir opp.
 *
 * ## ⚠️ Spillehodet som frøs
 *
 * `startPlay` satte `isPlaying`/`playing` FØR den ventet på `canplay`, og hvis
 * det eventet aldri kom, kom heller ingenting annet: `el.play()` ble aldri
 * kalt, `currentSec()` svarte `playStartSec` så lenge `mainPlayPending` sto,
 * og `animate()` gikk i ring på en rAF som malte det samme tallet for alltid.
 * Skjermen viste «pause»-knappen over en teller som ikke beveget seg, og det
 * er en tilstand ingenting i appen kunne komme ut av uten et nytt klikk.
 *
 * Ti sekunder er lenge nok til at en 90-minutters FLAC på en treg disk rekker
 * å melde metadata, og kort nok til at ingen står og lurer. Den ekte
 * bakstopperen er lasteren, som setter `playbackSource = "none"` når den vet
 * at fila ikke lar seg spille; dette er beltet under den, for det den ikke
 * kan vite på forhånd.
 */
export const PLAY_READY_TIMEOUT_MS = 10_000;

/**
 * Hvert start/stopp øker denne. Regionoverganger er asynkrone (`canplay`,
 * `ended`), og en callback fra en AVLØST avspilling må ikke gjøre noe — å
 * trykke stopp og så play innenfor samme sekund lot ellers den gamle
 * `canplay`-en starte elementet på nytt oppå den nye.
 */
let playGen = 0;

/** Vakten under `canplay`. Se `PLAY_READY_TIMEOUT_MS`. */
let readyTimer: ReturnType<typeof setTimeout> | null = null;

function clearReadyTimer(): void {
  if (!readyTimer) return;
  clearTimeout(readyTimer);
  readyTimer = null;
}

let persistentPlayerEl: HTMLAudioElement | null = null;

/** Elementet, laget ved første behov. */
export function ensurePlayerEl(): HTMLAudioElement {
  if (!persistentPlayerEl) {
    const el = new Audio();
    el.preload = "auto";
    persistentPlayerEl = el;
  }
  E.playerEl = persistentPlayerEl;
  return persistentPlayerEl;
}

/** Fortell modellen hvordan avspillingen faktisk går. */
export function setPlaybackSource(source: PlaybackSource): void {
  E.playbackSource = source;
  playbackSource.value = source;
}

/**
 * Slipp den åpne filas transport: pause, dropp `src` så webviewen lukker
 * filhåndtaket (og en midlertidig mellomfil kan feies bort). Elementet
 * beholdes for gjenbruk.
 */
export function teardownPlayback(): void {
  stopPlay();
  const el = E.playerEl;
  E.playerEl = null;
  setPlaybackSource("original");
  if (!el) return;
  try {
    el.pause();
  } catch {
    /* allerede pauset */
  }
  el.volume = 1;
  el.removeAttribute("src");
  try {
    el.load();
  } catch {
    /* ingen kilde å laste */
  }
}

/**
 * Flytt et elements posisjon, og tål en ressurs som ikke er åpen ennå.
 *
 * WebKit kaster `InvalidStateError` når `currentTime` skrives mens
 * `readyState` er HAVE_NOTHING, og hvert eneste søk i editoren lander på en
 * strømmet `<audio>` som kanskje fortsatt åpner. Et unntak her ville avbrutt
 * håndtereren som søkte, og tatt spillehodets tegning med seg.
 */
function seekEl(el: HTMLMediaElement, sec: number): void {
  try {
    el.currentTime = sec;
  } catch {
    /* ikke åpen ennå — E.playStartSec holder fortsatt målet */
  }
}

/** Der spillehodet er NÅ. */
function currentSec(): number {
  if (!E.isPlaying) return E.playStartSec;
  // Play er bedt om, men elementet har ikke startet: `currentTime` er
  // fortsatt 0, og å lese den ville visket ut brukerens søk.
  if (E.mainPlayPending) return E.playStartSec;
  const el = E.playerEl;
  return el ? el.currentTime : E.playStartSec;
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
  const el = E.playerEl;
  if (el) seekEl(el, E.playStartSec);
  scheduleDraw();
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
  cancelDuck();
  const el = E.playerEl;
  // Ingen transport i det hele tatt: ikke lat som. Knappen er sperret med en
  // grunn i UI-et, og dette er beltet under den.
  if (!el || !el.getAttribute("src")) return;

  E.playStartSec = snapOutOfCut(clampToFile(E.playStartSec));
  E.isPlaying = true;
  E.mainPlayPending = true;
  playing.value = true;

  const start = (): void => {
    clearReadyTimer();
    if (gen !== playGen || !E.isPlaying) return;
    E.mainPlayPending = false;
    seekEl(el, E.playStartSec);
    void el.play().catch(() => {});
  };

  attachEnded(() => {
    if (gen !== playGen || !E.isPlaying) return;
    stopPlay();
  });

  if (el.readyState >= 1) {
    start();
  } else {
    el.addEventListener("canplay", start, { once: true });
    // Vakten. Uten den er «`canplay` kom aldri» et frosset spillehode uten
    // utgang; med den blir det den setningen skjermen allerede har for
    // nøyaktig dette (`editor.qualityFallback`, gjennom `playbackSource`), og
    // en play-knapp som er av med en grunn i stedet for på med en løgn.
    readyTimer = setTimeout(() => {
      readyTimer = null;
      if (gen !== playGen || !E.isPlaying || !E.mainPlayPending) return;
      console.warn(
        "[editor] `canplay` kom aldri innen",
        PLAY_READY_TIMEOUT_MS,
        "ms — avspilling gis opp",
      );
      el.removeEventListener("canplay", start);
      stopPlay();
      setPlaybackSource("none");
    }, PLAY_READY_TIMEOUT_MS);
  }

  resetDrawGate();
  animate();
}

export function stopPlay(): void {
  playGen++;
  clearReadyTimer();
  detachEnded();
  cancelDuck();
  const wasPlaying = E.isPlaying;
  const at = wasPlaying ? currentSec() : E.playStartSec;
  E.mainPlayPending = false;
  const el = E.playerEl;
  if (el) {
    try {
      el.pause();
    } catch {
      /* allerede pauset */
    }
    el.volume = 1;
  }
  if (wasPlaying) E.playStartSec = clampToFile(at);
  E.isPlaying = false;
  playing.value = false;
  cancelAnimationFrame(E.rafId);
  E.rafId = 0;
  publishPlayhead(true);
  scheduleDraw();
}

// ── Løkka ───────────────────────────────────────────────────────────────────

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

function animate(): void {
  if (!E.isPlaying) return;
  const at = currentSec();

  // Hopp over kuttene. Undertrykket mens rampen går, ellers ville
  // nedtoningen blitt startet på nytt av det kuttet den holder på å rømme fra.
  const el = E.playerEl;
  if (el && !E.mainPlayPending && !ducking) {
    const inside = E.cuts.find((c) => at >= c.start && at < c.end);
    if (inside) skipTo(el, Math.min(E.duration, inside.end));
  }

  E.playStartSec = at;
  publishPlayhead(false);
  followPlayhead(at);
  scheduleDraw();
  E.rafId = requestAnimationFrame(animate);
}

// ── De-klikket ──────────────────────────────────────────────────────────────

let duckToken = 0;
let ducking = false;

function cancelDuck(): void {
  duckToken++;
  ducking = false;
  const el = E.playerEl;
  if (el) el.volume = 1;
}

function skipTo(el: HTMLMediaElement, targetSec: number): void {
  const token = ++duckToken;
  ducking = true;
  let frame = 0;

  const fadeIn = (): void => {
    if (token !== duckToken) return;
    frame++;
    el.volume = Math.min(1, frame / DUCK_FRAMES);
    if (frame < DUCK_FRAMES) {
      requestAnimationFrame(fadeIn);
      return;
    }
    el.volume = 1;
    ducking = false;
  };

  const fadeOut = (): void => {
    if (token !== duckToken) return;
    frame++;
    el.volume = Math.max(0, 1 - frame / DUCK_FRAMES);
    if (frame < DUCK_FRAMES) {
      requestAnimationFrame(fadeOut);
      return;
    }
    seekEl(el, targetSec);
    E.playStartSec = targetSec;
    frame = 0;
    requestAnimationFrame(fadeIn);
  };

  requestAnimationFrame(fadeOut);
}

// ── `ended`, uten å samle opp døde lyttere ──────────────────────────────────

let endedHandler: (() => void) | null = null;

function attachEnded(onEnded: () => void): void {
  const el = E.playerEl;
  if (!el) return;
  if (endedHandler) el.removeEventListener("ended", endedHandler);
  endedHandler = onEnded;
  el.addEventListener("ended", onEnded, { once: true });
}

function detachEnded(): void {
  const el = E.playerEl;
  if (el && endedHandler) el.removeEventListener("ended", endedHandler);
  endedHandler = null;
}
