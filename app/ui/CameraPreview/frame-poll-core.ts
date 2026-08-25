/**
 * Løkka som henter kamerabilder mens et opptak går — uten en eneste DOM-node.
 *
 * Under et opptak eier bakendens ffmpeg kameraet, og den skriver en JPEG til en
 * fast sti ~12 ganger i sekundet. Overlegget POLLER den fila
 * (`recording_preview_frame`). Det er ikke en strøm, og det er med vilje: den
 * gamle Electron-appen fikk IPC-frames, Tauri-opptakeren skriver en fil, og en
 * poll er det som matcher.
 *
 * ## ⚠️ Vakten er hele grunnen til at fila finnes
 *
 * `setInterval` bryr seg ikke om at forrige tikk ikke er ferdig. Hvert tikk
 * venter på en IPC-rundtur og en base64-dekoding; på en treg disk eller en
 * opptatt bakende stabler de seg — tolv overlappende lesninger i sekundet, alle
 * i kappløp om å sette den samme `img.src`, alle på hovedtråden, midt i det
 * ene minuttet av uka som ikke kan tas om igjen. Et HOPPET bilde er usynlig ved
 * 12 fps; en kø av dem er det ikke.
 *
 * Løkka bor her og ikke i komponenten fordi vakten da kan bevises i node-gaten
 * med falske timere: `frame-poll-core.test.ts` fjerner vakten i tanken og viser
 * hva som skjer. Komponenten er bare `<img>`-en rundt.
 */

/** Bakendens preview-takt: 12 fps ⇒ ~83 ms. Legacys tall, og motorens. */
export const FRAME_POLL_MS = 83;

export interface FramePollOptions {
  /** Ett forsøk på å hente en frame. `null` = ingen ennå. */
  fetchFrame: () => Promise<string | null>;
  /** Kalles bare når det FAKTISK kom en frame. */
  onFrame: (b64: string) => void;
  /** Millisekunder mellom tikk. Default `FRAME_POLL_MS`. */
  intervalMs?: number;
}

/**
 * Start løkka. Returverdien stopper den — og den er idempotent, fordi en
 * `useEffect`-opprydding kan kjøre etter at komponenten alt er borte.
 *
 * Et tikk som feiler er stille: en enkelt tapt frame under et opptak er ikke
 * noe en frivillig skal få en feilmelding om, og en toast per tapte frame ville
 * vært tolv i sekundet.
 */
export function startFramePoll(opts: FramePollOptions): () => void {
  const interval = opts.intervalMs ?? FRAME_POLL_MS;
  let busy = false;
  let stopped = false;

  const timer = setInterval(() => {
    // ⚠️ VAKTEN. Se filhodet — fjernes denne, stabler kallene seg.
    if (busy) return;
    busy = true;
    void opts
      .fetchFrame()
      .then((b64) => {
        if (stopped || !b64) return;
        opts.onFrame(b64);
      })
      .catch(() => {
        /* én tapt frame er ikke en hendelse */
      })
      .finally(() => {
        busy = false;
      });
  }, interval);

  return () => {
    if (stopped) return;
    stopped = true;
    clearInterval(timer);
  };
}
