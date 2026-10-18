/**
 * ÉN kameraøkt i bakenden, mange visninger — reglene, uten DOM og uten Tauri.
 *
 * ## Hvorfor dette finnes
 *
 * Kamerabildet vises to steder som lever samtidig: kortet på Opptak, og
 * opptaksoverlegget som legger seg over det når Start trykkes. Begge vil ha
 * bildet av SAMME økt, og bakenden har én kanal for bildet. Hadde hver
 * komponent kalt `camera_preview_start` selv, ville den andre tatt kanalen fra
 * den første (og en økt på tomgang blitt lukket og åpnet på nytt — nøyaktig det
 * blinket C4b er til for å fjerne). Derfor eier DENNE modulen kallet: første
 * som ber om bildet starter økta, siste som slipper det stopper den, og
 * bildene deles ut til alle imens. Samme grep som VU-matingen
 * (`lib/audio/vu-feed.ts`).
 *
 * ## Kort frist før stopp
 *
 * Et sideskifte monterer den ene visningen av og den neste på, i den
 * rekkefølgen. Uten en frist ville siste-slipper-stopper lukket kameraet midt
 * i skiftet. `graceMs` lar neste visning ta over uten at bakenden merker noe.
 *
 * ## Rekkefølge
 *
 * `start` og `stop` kjøres etter hverandre, aldri om hverandre: et `stop` som
 * overtok et `start` på vei ville latt kameraet stå åpent uten noen som ser på.
 */

/** Hvor langt økta er kommet, sett fra en visning. */
export type FeedState = "idle" | "starting" | "live" | "denied" | "failed";

/** Hva bakenden svarte på «vis kameraet». */
export type StartResult = { ok: true } | { ok: false; error: string };

/** Det modulen trenger av bakenden (`window.api.cameraPreview*`). */
export interface FeedApi {
  start: (
    token: string,
    onFrame: (jpeg: ArrayBuffer) => void,
  ) => Promise<StartResult>;
  stop: () => Promise<void>;
}

export interface FeedTimers {
  set: (fn: () => void, ms: number) => unknown;
  clear: (handle: unknown) => void;
}

/** En visning som følger med. */
export interface FeedSubscriber {
  onFrame: (jpeg: ArrayBuffer) => void;
  onState: (state: FeedState) => void;
}

export interface CameraFeed {
  /** Be om bildet av kameraet `token`. Returverdien slipper det igjen. */
  acquire: (token: string, sub: FeedSubscriber) => () => void;
  /** Tilstanden nå (for tester og diagnostikk). */
  state: () => FeedState;
  /**
   * Tilstand og siste bilde nå. En visning som monteres midt i en økt leser
   * dette i sin FØRSTE render, så den ikke engang et øyeblikk står tom.
   */
  snapshot: () => { state: FeedState; frame: ArrayBuffer | null };
}

/** Frist mellom siste slipp og stopp. */
export const FEED_GRACE_MS = 400;

export function createCameraFeed(
  api: FeedApi,
  classify: (error: string) => "denied" | "other",
  timers: FeedTimers,
  graceMs: number = FEED_GRACE_MS,
): CameraFeed {
  const subs = new Set<FeedSubscriber>();
  let state: FeedState = "idle";
  /** Kameraet økta er bedt om. `null` = ingen økt. */
  let token: string | null = null;
  /** Telles opp for hver start; bilder fra en eldre kanal slippes. */
  let generation = 0;
  let stopTimer: unknown = null;
  /**
   * Siste bilde. En visning som kommer til underveis (overlegget, midt i Start)
   * får det med én gang, i stedet for å stå tom til neste bilde — nøyaktig det
   * ville vært blinket C4b er til for å fjerne.
   */
  let lastFrame: ArrayBuffer | null = null;
  let chain: Promise<void> = Promise.resolve();

  const setState = (next: FeedState): void => {
    if (state === next) return;
    state = next;
    for (const sub of [...subs]) sub.onState(next);
  };

  /** Kjør `job` etter alt som står i kø, og svelg feilen: køen skal leve. */
  const enqueue = (job: () => Promise<void>): void => {
    chain = chain.then(job).catch(() => undefined);
  };

  const begin = (wanted: string): void => {
    token = wanted;
    lastFrame = null;
    const mine = ++generation;
    setState("starting");
    enqueue(async () => {
      if (mine !== generation) return;
      const result = await api.start(wanted, (jpeg) => {
        if (mine !== generation) return;
        // Det første bildet er beviset på at økta lever.
        lastFrame = jpeg;
        setState("live");
        for (const sub of [...subs]) sub.onFrame(jpeg);
      });
      if (mine !== generation) return;
      if (!result.ok) {
        token = null;
        setState(classify(result.error) === "denied" ? "denied" : "failed");
      }
    });
  };

  const end = (): void => {
    stopTimer = null;
    if (subs.size > 0 || token === null) return;
    token = null;
    lastFrame = null;
    generation++;
    setState("idle");
    enqueue(() => api.stop());
  };

  return {
    state: () => state,
    snapshot: () => ({ state, frame: lastFrame }),
    acquire(wanted, sub) {
      subs.add(sub);
      if (stopTimer !== null) {
        timers.clear(stopTimer);
        stopTimer = null;
      }
      sub.onState(state);
      if (lastFrame !== null) sub.onFrame(lastFrame);
      // En økt som feilet eller står på et annet kamera startes på nytt;
      // en som lever (eller er på vei) deles.
      if (token !== wanted) begin(wanted);
      return () => {
        if (!subs.delete(sub)) return;
        if (subs.size === 0 && token !== null) {
          stopTimer = timers.set(end, graceMs);
        }
      };
    },
  };
}
