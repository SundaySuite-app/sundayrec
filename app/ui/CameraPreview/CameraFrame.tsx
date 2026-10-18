/**
 * Rammen rundt kamerabildet — før og under opptaket, samme flate.
 *
 * Bildet kommer fra bakendens kameraøkt både på Opptak og i overlegget
 * (`CameraPreview.tsx`); en frivillig som har sett bildet på Opptak skal
 * kjenne igjen det samme bildet i overlegget. Derfor eier denne komponenten
 * alt som er FELLES: sideforholdet (og dermed at flaten ikke hopper når det
 * første bildet lander), plassholderteksten og speilvendingen.
 *
 * ## `data-phase` er kontrakten utover
 *
 * e2e leser fasen, ALDRI bildet. En journeytest som venter på piksler venter på
 * maskinvare i en nettleser uten kamera; en som venter på `data-phase="live"`
 * venter på appens egen påstand om at bildet er der. Den første er flakete,
 * den andre er en test.
 */

import type { ComponentChildren } from "preact";

import type { CameraPhase } from "./camera-preview-core";
import styles from "./CameraFrame.module.css";

export interface CameraFrameProps {
  /** Grovtilstanden. Havner på `data-phase` og styrer om barnet vises. */
  phase: CameraPhase;
  /** Setningen over plassholderen. Utelates når bildet er der. */
  message?: string;
  /**
   * Sideforholdet som `w / h`, når det er kjent. Settes som
   * `--rec-video-ar`, så rammen har riktig høyde FØR første bilde er dekodet.
   * Uten den står 16:9 fra CSS-en, som er det opptaket leverer.
   */
  aspect?: string | null;
  /** Speilvend bildet, som opptaket (`videoFlip`). Havner på `data-flip`. */
  flip?: boolean;
  /** `<img>`. Skjult av CSS til fasen er `live`. */
  children?: ComponentChildren;
  testId?: string;
}

export function CameraFrame({
  phase,
  message,
  aspect,
  flip,
  children,
  testId,
}: CameraFrameProps) {
  return (
    <div
      data-testid={testId}
      data-phase={phase}
      data-flip={flip ? "true" : undefined}
      class={styles.frame}
      style={aspect ? `--rec-video-ar: ${aspect}` : undefined}
    >
      {children}
      {message ? (
        <p
          data-testid={testId ? `${testId}-message` : undefined}
          class={styles.message}
        >
          {message}
        </p>
      ) : null}
    </div>
  );
}
