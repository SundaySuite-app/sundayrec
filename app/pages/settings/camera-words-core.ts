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

/**
 * Bilder i sekundet → ordet. 50+ er svært jevnt, ~24+ er jevnt (film, 25/30).
 * Grensen er 23.9, ikke 24: film kommer som 23.976, og den skal ikke kalles
 * hakkete. Ukjent (0, negativ, NaN) gir `null` — da sier vi ingenting om
 * bevegelse i stedet for å påstå at kameraet er hakkete.
 */
export function motionWord(maxFps: number): MotionWord | null {
  if (!Number.isFinite(maxFps) || maxFps <= 0) return null;
  if (maxFps >= 50) return "smooth";
  if (maxFps >= 23.9) return "normal";
  return "low";
}
