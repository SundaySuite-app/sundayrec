/**
 * Steg 2 — LYD. Tilstanden, analysen og lyttingen.
 *
 * Tabellen som gjør de tre ordene om til tall bor i `sound-profiles.ts` og er
 * ren. Her er alt som har en bivirkning: et signal per ting treet viser, ÉN
 * memoisert kanalanalyse per fil, og lyttingen på avspillingsmotoren.
 *
 * ## «Før» og «Etter» er en bryter, ikke en gjengivelse (B5b)
 *
 * Lyttingen gikk før gjennom `editor_master_preview`: en tjuesekunders mp3 gjengitt
 * av ffmpeg for hvert valg, spilt av i et eget `<audio>`. Nå spiller Rust opptaket
 * selv, på lydkortet, med kuttene (`playback/`, B5a), og har BÅDE programmet slik
 * det er («Før») og programmet gjennom kjeden («Etter») klare i det samme øyeblikket.
 * Å velge side er en melding som lydtråden krysstoner på 20 ms; ingenting gjengis og
 * ingen fil skrives. Spillehodet er det motoren sier (`editor-listen`, `Position`),
 * ikke noe vi regner ut her.
 *
 * To ting venter fortsatt, og begge er målingen av programmet, ikke lyden:
 *
 *   - kjeden for «Etter» trenger nivået eksporten ville gitt programmet (ett helt
 *     kjedepass, første gang for hvert valg og hver kuttliste). `watchListenPrepare`
 *     gjør den ferdig i bakgrunnen mens steget er åpent, så den som trykker «Etter»
 *     som regel ikke venter;
 *   - «Før» venter på ingenting: økta åpnes uten kjede, og kjeden settes inn når den er klar.
 *
 * Bytter man profil eller mikser mens det spiller, bygger motoren den nye kjeden ved
 * siden av den gamle og krysstoner (`playback/producer.rs`): ingen klikk, ingen pause,
 * og den nye høres etter høyst et halvt sekund.
 *
 * ## Økta
 *
 * Én økt om gangen, åpnet av første «spill» og lukket når steget forlates eller fila
 * byttes (`stopListen`). Økta eier lydkortet, så den holdes ikke åpen lenger enn steget.
 */

import { computed, effect, signal } from "@preact/signals";
import type {
  ExportChannelRepair,
  MasterPreviewParams,
} from "@lib/pages/editor/export-params";
import { defaultProcessing, type Processing } from "@lib/pages/editor/mixer";

import type { EditorTaleSaid } from "@legacy/bindings/EditorTaleSaid";
import { isRecording } from "../state/recording";
import type { EditorListenEvent } from "@legacy/bindings/EditorListenEvent";
import { cuts, E } from "./model";
import { sermonWindow } from "./editor-core";
import {
  DEFAULT_SOUND_PROFILE,
  LISTEN_SPAN_SEC,
  listenStartSec,
  soundExportFields,
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
/** B3b: hva «Tale» gjorde med fila — kodene fra `editor_tale_said`, eller `null`
 *  før svaret er inne (og når analysen ikke kunne kjøres). */
export const taleSaid = signal<EditorTaleSaid | null>(null);

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
  // Lappen, ikke stien: Rust slår opp opptaket med den (A2).
  const token = E.sourceToken;
  if (!token) return Promise.resolve();
  if (analysisFor === token && analysis) return analysis;
  analysisFor = token;
  analyzingSound.value = true;
  // «Tale»-analysen (B3b) kjører ved siden av kanalanalysen og deler cache med
  // forhåndslytting og eksport. Den får ikke holde igjen kanalnotatet, og en
  // feil her er bare en setning som uteblir.
  void (async () => {
    try {
      const said = await window.api.editorTaleSaid(token);
      if (E.sourceToken === token) taleSaid.value = said;
    } catch {
      /* ingen forklaring er et gyldig svar */
    }
  })();
  analysis = (async () => {
    try {
      const res = await window.api.editorAutoProcess(token);
      // En annen fil rakk å bli åpnet mens vi analyserte: svaret gjelder
      // ikke lenger noe som står på skjermen.
      if (E.sourceToken !== token) return;
      const rec = res?.diagnosis?.recommended;
      channelCode.value = res?.diagnosis?.code ?? null;
      channelRepair.value =
        rec && rec.mode && rec.mode !== "none"
          ? { mode: rec.mode, leftDb: rec.leftDb, rightDb: rec.rightDb }
          : null;
    } catch {
      /* ingen anbefaling er et gyldig svar */
    } finally {
      if (E.sourceToken === token) analyzingSound.value = false;
    }
  })();
  return analysis;
}

// ── Før/etter-lyttingen ─────────────────────────────────────────────────────

export type ListenSide = "before" | "after";

export const listenSide = signal<ListenSide>("after");
export const listenPlaying = signal(false);
export const listenError = signal(false);
/** Økta er åpen og kjeden for det som er valgt NÅ er ikke inne i den ennå: «Etter» venter
 *  på målingen av programmet (første gang for dette valget og disse kuttene), ikke på lyd.
 *  Teksten under knappen sier det. */
const listenStale = signal(false);
export const listenPreparing = computed(
  () => listenStale.value && listenSide.value === "after",
);

/** Hvor lyttingen begynner (kildesekunder): midt i prekenen. Linja «0:21:08 — fra prekenen». */
export const listenStart = signal(0);
/** Spillehodet slik motoren melder det (kildesekunder), eller `null` før første melding. */
export const listenAt = signal<number | null>(null);
/** Hull i lyden motoren har telt (ringen gikk tom mens det spilte). Til diagnose. */
export const listenUnderruns = signal(0);

/** Regn ut hvor lyttingen begynner, ut fra prekenvinduet slik det står NÅ. */
export function refreshListenStart(): void {
  const window = sermonWindow({
    cuts: E.cuts,
    duration: E.duration,
    suggestion: E.suggestion,
    applied: E.applied,
  });
  listenStart.value = listenStartSec(window, E.duration);
}

/** Økta, når den er åpen: hvilken fil den hører til, og avmeldingen fra motorens hendelser. */
let session: { token: string; unlisten: () => void } | null = null;
let opening: Promise<boolean> | null = null;
let openSeq = 0;
let closing: Promise<void> | null = null;
/** Høyeste generasjon sett: en posisjon fra en eldre er forsinket og ignoreres. */
let maxGen = 0;
/** Programmet er spilt ut: neste «spill» begynner på nytt. */
let atEnd = false;
/** Nøkkelen til kjeden som ligger i motoren, og telling av forespørsler om en ny. */
let installedKey: string | null = null;
let chainSeq = 0;
let chainInflight: { key: string; promise: Promise<ChainResult> } | null = null;

type ChainResult = "ok" | "superseded" | "failed";

/** Stopp lyttingen og slipp lydkortet. Kalles når steget forlates og når fila lukkes. */
export function stopListen(): void {
  const had = session;
  session = null;
  opening = null;
  openSeq++;
  atEnd = false;
  installedKey = null;
  maxGen = 0;
  chainSeq++;
  chainInflight = null;
  listenPlaying.value = false;
  listenStale.value = false;
  listenAt.value = null;
  if (!had) return;
  had.unlisten();
  // Neste åpning venter på denne: lydkortet har én eier om gangen.
  closing = Promise.resolve(window.api.editorListenClose()).then(() => {
    closing = null;
  });
}

function onListenEvent(raw: unknown): void {
  const e = raw as EditorListenEvent;
  switch (e.kind) {
    case "position":
      if (e.gen < maxGen) return;
      maxGen = e.gen;
      listenAt.value = e.sourceSecs;
      return;
    case "ended":
      listenPlaying.value = false;
      atEnd = true;
      return;
    case "underrun":
      listenUnderruns.value = e.total;
      return;
    case "error":
      listenError.value = true;
      listenPlaying.value = false;
      return;
    default:
      return;
  }
}

/** Forespørselen uten lyd: kuttene og lengden og hvor man begynner. Det økta åpnes med. */
function bareRequest(start: number): MasterPreviewParams {
  return {
    cutRegions: E.cuts.map((c) => ({ start: c.start, end: c.end })),
    fullLengthSec: E.duration,
    startSec: start,
    durationSec: LISTEN_SPAN_SEC,
  };
}

/** Åpne økta hvis den ikke er åpen. Venter ikke på noe annet enn at lydkortet er åpnet. */
async function ensureSession(): Promise<boolean> {
  const token = E.sourceToken;
  if (!token) return false;
  if (session && session.token === token) return true;
  if (opening) return opening;
  const mine = ++openSeq;
  const run = (async () => {
    if (closing) await closing;
    const res = await window.api.editorListenOpen(
      token,
      listenRequest(listenStart.peek()) ?? bareRequest(listenStart.peek()),
    );
    // Steget ble forlatt, eller en annen fil åpnet, mens lydkortet åpnet.
    if (openSeq !== mine || E.sourceToken !== token) {
      if (res.ok && !session) void window.api.editorListenClose();
      return false;
    }
    if (!res.ok) return false;
    maxGen = 0;
    atEnd = false;
    installedKey = null;
    listenAt.value = listenStart.peek();
    session = {
      token,
      unlisten: window.api.on("editor-listen", onListenEvent) ?? (() => {}),
    };
    listenStale.value = true;
    return true;
  })().finally(() => {
    if (openSeq === mine) opening = null;
  });
  opening = run;
  return run;
}

/** Nøkkelen til et valg: forespørselen uten hvor man lytter, for den avgjør kjeden. */
function chainKey(request: MasterPreviewParams): string {
  return programKey(request);
}

/**
 * Sett kjeden for det som er valgt nå inn i økta. Måler programmet først hvis det ikke er gjort
 * (delt med bakgrunnsmålingen og eksporten). En nyere forespørsel går foran en eldre som blir
 * ferdig senere. Når kjeden er inne og «Etter» er valgt, velges «Etter» i motoren — det er
 * da bryteren skjer, og den er øyeblikkelig.
 */
async function installChain(): Promise<ChainResult> {
  await ensureSoundAnalysis();
  const token = E.sourceToken;
  const request = listenRequest(listenStart.peek());
  if (!session || !token || !request) return "failed";
  const key = chainKey(request);
  // Samme forespørsel er allerede på vei: vent på den i stedet for å bestille den en gang til.
  if (chainInflight && chainInflight.key === key) return chainInflight.promise;
  if (!chainInflight && installedKey === key) {
    listenStale.value = false;
    await applySide();
    return "ok";
  }
  // En annen forespørsel er på vei: den ville lagt seg over denne i motoren om den ble ferdig
  // etter, så også et valg motoren allerede har bestilles på nytt (nyere forespørsel vinner).
  const my = ++chainSeq;
  listenStale.value = true;
  const promise = (async (): Promise<ChainResult> => {
    const ok = await window.api.editorListenChain(token, request);
    if (my !== chainSeq) return "superseded";
    if (!ok) {
      listenError.value = true;
      return "failed";
    }
    installedKey = key;
    listenStale.value = false;
    await applySide();
    return "ok";
  })();
  chainInflight = { key, promise };
  void promise.finally(() => {
    if (chainInflight?.promise === promise) chainInflight = null;
  });
  return promise;
}

/** Velg siden i motoren: «Etter» bare når kjeden for valget er inne. */
async function applySide(): Promise<void> {
  if (!session) return;
  const request = listenRequest(listenStart.peek());
  const ready = request !== null && installedKey === chainKey(request);
  await window.api.editorListenTransport({
    kind: "side",
    after: listenSide.peek() === "after" && ready,
  });
}

/** Velg side. Er økta åpen og kjeden inne, er det hørt med en gang. */
export function setListenSide(side: ListenSide): void {
  if (listenSide.value === side) return;
  listenSide.value = side;
  listenError.value = false;
  if (!session) return;
  if (side === "before") {
    void applySide();
    return;
  }
  void installChain();
}

/**
 * Spill, eller pause.
 *
 * Økta åpnes av det første «spill». «Etter» venter på at kjeden er inne (målingen av
 * programmet, første gang); «Før» venter ikke. Etter en pause fortsetter det der det stod,
 * og etter at programmet er spilt ut begynner det på nytt der lyttingen begynner.
 */
export async function toggleListen(): Promise<void> {
  if (listenPlaying.value) {
    listenPlaying.value = false;
    await window.api.editorListenTransport({ kind: "pause" });
    return;
  }
  listenError.value = false;
  if (!(await ensureSession())) {
    listenError.value = true;
    return;
  }
  // Kjeden settes inn for begge sider: «Før» spiller med en gang og «Etter» er klar når man trykker.
  const chain = installChain();
  if (listenSide.peek() === "after") {
    let result = await chain;
    while (result === "superseded" && chainInflight) {
      result = await chainInflight.promise;
    }
    // Steget ble forlatt mens kjeden ventet: det er ikke en feil.
    if (!session) return;
    if (result !== "ok") {
      listenError.value = true;
      return;
    }
  }
  if (!session) return;
  if (atEnd) {
    atEnd = false;
    maxGen = 0;
    await window.api.editorListenTransport({
      kind: "seekSource",
      secs: listenStart.peek(),
    });
  }
  await window.api.editorListenTransport({ kind: "play" });
  listenPlaying.value = true;
}

/**
 * Forespørselen kjeden bygges av: lydfeltene fra `soundExportFields` — den ENE
 * funksjonen eksporten også leser lyd-steget gjennom — og kuttene og lengden
 * eksporten sender, slik at bakenden kan måle programmet. `null` når valget ikke
 * gir noe å høre («Ingen»).
 */
export function listenRequest(start: number): MasterPreviewParams | null {
  const fields = soundExportFields({
    profile: soundProfile.value,
    useMixer: useMixer.value,
    processing: useMixer.value ? mixerProcessing() : undefined,
    repair: channelRepair.value,
  });
  if (!fields.masterPreset && !fields.processing) return null;
  return {
    presetId: fields.masterPreset,
    processing: fields.processing,
    channelRepair: fields.channelRepair,
    cutRegions: E.cuts.map((c) => ({ start: c.start, end: c.end })),
    fullLengthSec: E.duration,
    startSec: start,
    durationSec: LISTEN_SPAN_SEC,
  };
}

// ── Lyttingen gjøres klar i bakgrunnen ──────────────────────────────────────

/** Ventetid etter siste endring før målingen starter. En glidebryter i mikseren gir mange
 *  endringer, og hver av dem ville ellers startet (og avbrutt) et helt kjedepass. */
export const PREPARE_DEBOUNCE_MS = 800;

/** Programmene bakenden har målt ferdig, og målingene som pågår. Nøkkelen er forespørselen
 *  UTEN utsnittet: samme valg og samme kutt er samme program, uansett hvor man lytter. */
const readyPlans = new Set<string>();
const pendingPlans = new Map<string, Promise<boolean>>();
let prepareTimer = 0;

function programKey(request: MasterPreviewParams): string {
  const { startSec: _start, durationSec: _span, ...program } = request;
  return `${E.sourceToken}|${JSON.stringify(program)}`;
}

/** Be bakenden måle programmet nå — med mindre det er målt, pågår, eller et opptak går.
 *  Bakenden vokter det siste også (opptak og eksport går foran), men det er billigere å ikke
 *  spørre. Er økta åpen når målingen er ferdig, settes den nye kjeden inn i den. */
async function prepareNow(): Promise<void> {
  if (isRecording.peek()) return;
  const token = E.sourceToken;
  if (!token) return;
  const request = listenRequest(listenStart.peek());
  if (!request) {
    // «Ingen»: det er ingenting å høre «Etter» på. Kortet er borte, og lyttingen med det.
    if (session) stopListen();
    return;
  }
  const pk = programKey(request);
  if (!readyPlans.has(pk)) {
    let run = pendingPlans.get(pk);
    if (!run) {
      const started = (async () => {
        try {
          const ready = await window.api.prepareListen(token, request);
          if (ready && E.sourceToken === token) readyPlans.add(pk);
          return ready;
        } catch {
          return false;
        } finally {
          pendingPlans.delete(pk);
        }
      })();
      pendingPlans.set(pk, started);
      run = started;
    }
    await run;
  }
  if (session && readyPlans.has(pk) && E.sourceToken === token) {
    void installChain();
  }
}

/** Planlegg en måling om `delayMs`; en ny planlegging erstatter den forrige. Kanalanalysen
 *  avgjør forespørselen (reparasjonen er en del av den), så målingen venter på den. */
export function scheduleListenPrepare(delayMs: number): void {
  cancelListenPrepare();
  if (isRecording.peek() || !E.sourceToken) return;
  prepareTimer = window.setTimeout(() => {
    prepareTimer = 0;
    void ensureSoundAnalysis().then(prepareNow, prepareNow);
  }, delayMs);
}

export function cancelListenPrepare(): void {
  window.clearTimeout(prepareTimer);
  prepareTimer = 0;
}

/**
 * Hold «Etter» klar mens steget er åpent: mål programmet med en gang, og på nytt (med
 * ventetid) når valget, mikseren, kanalreparasjonen eller kuttene endres. Under et opptak
 * startes ingenting, og når opptaket er over, tas det igjen. Er økta åpen, settes den nye
 * kjeden inn når den er målt — det er det som gjør profilbytte midt i avspilling.
 * Returnerer en funksjon som avslutter overvåkingen; en måling som alt pågår i bakenden får
 * gå ferdig.
 */
export function watchListenPrepare(): () => void {
  let first = true;
  const dispose = effect(() => {
    // Det som avgjør forespørselen. `cuts` er speilet av `E.cuts`; resten er signaler.
    void [
      soundProfile.value,
      useMixer.value,
      mixer.value,
      channelRepair.value,
      cuts.value,
      isRecording.value,
    ];
    // Spiller økta den gamle kjeden, er den nye ikke inne før målingen er ferdig.
    if (session) listenStale.value = true;
    scheduleListenPrepare(first ? 0 : PREPARE_DEBOUNCE_MS);
    first = false;
  });
  return () => {
    dispose();
    cancelListenPrepare();
  };
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
  taleSaid.value = null;
  analyzingSound.value = false;
  listenSide.value = "after";
  listenError.value = false;
  listenStart.value = 0;
  listenUnderruns.value = 0;
  cancelListenPrepare();
  readyPlans.clear();
  pendingPlans.clear();
  analysisFor = "";
  analysis = null;
}
