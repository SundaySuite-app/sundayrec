/**
 * Hva kameraet leverer, i ord en frivillig forstår.
 *
 * «Kameraet leverer maks 1080p · 30 bilder i sekundet» er to tall som bare
 * betyr noe for den som allerede kan det. Her blir de til «Full HD, jevn
 * bevegelse». Rent oppslag på høyde og bilderate, så grensene kan prøves uten
 * DOM — og så teksten står i katalogen (`app.setup.camera.res.*` og
 * `.motion.*`), ikke her.
 */

export type ResolutionWord = "uhd" | "fhd" | "hd" | "sd";
export type MotionWord = "smooth" | "normal" | "low";

/** Høyden i piksler → ordet. 1080p og opp er «Full HD», 4K er 2160p. */
export function resolutionWord(maxHeight: number): ResolutionWord {
  if (maxHeight >= 2160) return "uhd";
  if (maxHeight >= 1080) return "fhd";
  if (maxHeight >= 720) return "hd";
  return "sd";
}

/** Bilder i sekundet → ordet. 50+ er svært jevnt, 24+ er jevnt (film, 25/30). */
export function motionWord(maxFps: number): MotionWord {
  if (maxFps >= 50) return "smooth";
  if (maxFps >= 24) return "normal";
  return "low";
}
