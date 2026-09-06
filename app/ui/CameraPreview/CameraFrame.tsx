/**
 * Rammen rundt kamerabildet — den ENE, for begge kildene.
 *
 * Det er to helt forskjellige måter å få et bilde på i denne appen: FØR et
 * opptak er det webviewets egen `getUserMedia`-strøm i en `<video>`, og MENS
 * opptaket går er det bakendens preview-JPEG i en `<img>` (motoren eier
 * kameraet da, se `ownership.ts`). Kilden er ulik; flaten skal ikke være det.
 * En frivillig som har sett bildet på Opptak skal kjenne igjen det samme
 * bildet i overlegget.
 *
 * Derfor eier denne komponenten alt som er FELLES: sideforholdet (og dermed at
 * flaten ikke hopper når det første bildet lander), plassholderteksten, og
 * merket som sier hva kameraet faktisk leverer. Barnet er `<video>` eller
 * `<img>`, og det er hele forskjellen.
 *
 * ## `data-phase` er kontrakten utover
 *
 * e2e leser fasen, ALDRI bildet. En journeytest som venter på levende video
 * venter på maskinvare i en nettleser uten kamera; en som venter på
 * `data-phase="live"` venter på appens egen påstand om at strømmen er festet.
 * Den første er flakete, den andre er en test.
 */

import type { ComponentChildren } from "preact";

import type { CameraPhase } from "./live-preview-core";
import styles from "./CameraFrame.module.css";

export interface CameraFrameProps {
  /** Grovtilstanden. Havner på `data-phase` og styrer om barnet vises. */
  phase: CameraPhase;
  /** Setningen over plassholderen. Utelates når bildet er der. */
  message?: string;
  /** «1920×1080 · 30 fps» — hva som FAKTISK kom, ikke hva som ble bedt om. */
  badge?: string | null;
  /**
   * Sideforholdet som `w / h`, når det er kjent. Settes som
   * `--rec-video-ar`, så rammen har riktig høyde FØR første bilde er dekodet.
   * Uten den står 16:9 fra CSS-en, som er det opptaket leverer.
   */
  aspect?: string | null;
  /** `<video>` eller `<img>`. Skjult av CSS til fasen er `live`. */
  children?: ComponentChildren;
  testId?: string;
}

export function CameraFrame({
  phase,
  message,
  badge,
  aspect,
  children,
  testId,
}: CameraFrameProps) {
  return (
    <div
      data-testid={testId}
      data-phase={phase}
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
      {badge ? (
        <span
          data-testid={testId ? `${testId}-badge` : undefined}
          class={styles.badge}
        >
          {badge}
        </span>
      ) : null}
    </div>
  );
}
