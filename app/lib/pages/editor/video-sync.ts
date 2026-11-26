/**
 * Den dempede `<video>`-en følger avspillingsmotoren — beslutningen, uten DOM.
 *
 * Lyden spilles av Rust (se `app/editor/playback.ts`); bildet er en stum
 * `<video>` som holdes på samme sted. Motoren melder posisjonen ~30 ganger i
 * sekundet og hopper over kutt, så bildet må både følge løpende og hoppe
 * etter.
 *
 *   - Står det stille: bildet står på spillehodet (nøyaktig nok til at et
 *     enkeltbilde stemmer — `SEEK_EPS_SEC`), og videoen er satt på pause.
 *   - Spilles det: videoen spiller selv, og rettes bare når den har drevet
 *     mer enn `DRIFT_SEC` fra motoren. Å sette `currentTime` ved hver melding
 *     ville gitt et bilde som hakker; å la den gå fritt gir et bilde som
 *     sklir fra lyden over et kutt.
 */

/** Drift (sekunder) videoen får før den rettes mens det spilles. */
export const DRIFT_SEC = 0.25;

/** Så mye kan et stille bilde ligge fra spillehodet uten at det flyttes. */
export const SEEK_EPS_SEC = 0.04;

export interface VideoSyncInput {
  /** `video.currentTime`. */
  videoTime: number;
  /** Der motoren sier at vi er. */
  target: number;
  /** Spiller motoren? */
  playing: boolean;
  /** `video.paused`. */
  videoPaused: boolean;
}

export interface VideoSyncAction {
  /** Sett `currentTime` hit, eller `null` = la den stå. */
  seek: number | null;
  play: boolean;
  pause: boolean;
}

export function videoSyncAction(input: VideoSyncInput): VideoSyncAction {
  const { videoTime, target, playing, videoPaused } = input;
  const off = Math.abs(videoTime - target);
  if (!playing) {
    return {
      seek: off > SEEK_EPS_SEC ? target : null,
      play: false,
      pause: !videoPaused,
    };
  }
  return {
    seek: off > DRIFT_SEC ? target : null,
    play: videoPaused,
    pause: false,
  };
}
