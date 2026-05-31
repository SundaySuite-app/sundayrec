/** Pure peaks→SVG-geometry helpers for the editor waveform. Kept out of the
 *  component module so the mapping is unit-testable and React-fast-refresh
 *  stays happy (a component file should export only components). */

/**
 * Build the SVG `points` for a peaks polygon mirrored about the centre, so the
 * renderer paints a classic symmetric waveform band. `peaks` are 0..1 max-abs
 * amplitudes; `width`/`height` are the viewBox dimensions. Pure (no DOM), so
 * the data→geometry mapping is tested even though the `<svg>` paint is GUI-only.
 */
export function waveformPath(
  peaks: number[],
  width: number,
  height: number,
): string {
  if (peaks.length === 0) return "";
  const mid = height / 2;
  const dx = peaks.length > 1 ? width / (peaks.length - 1) : width;
  // Top edge left→right, then bottom edge right→left, closing a filled band.
  const top = peaks.map(
    (p, i) => `${(i * dx).toFixed(1)},${(mid - p * mid).toFixed(1)}`,
  );
  const bottom = peaks
    .map((_p, i) => {
      const j = peaks.length - 1 - i;
      return `${(j * dx).toFixed(1)},${(mid + peaks[j]! * mid).toFixed(1)}`;
    })
    .join(" ");
  return `${top.join(" ")} ${bottom}`;
}
