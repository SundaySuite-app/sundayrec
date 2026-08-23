/**
 * Editor haptics — the trackpad clicks when an edit locks into place.
 *
 * `haptic_perform` (commands/haptics.rs) has existed since the port, maps three
 * logical patterns onto Apple's `NSHapticFeedbackPattern`, and has never been
 * called from anywhere. Dragging a trim handle onto a detected speech boundary
 * is exactly the moment a Mac is supposed to click under your finger, and it
 * was silent.
 *
 * Two rules make this feel like a system feature rather than a gimmick:
 *
 *   1. Only fire when something actually SNAPPED — the value changed because a
 *      boundary claimed it, not merely because the mouse moved. `snapPulse`
 *      compares the raw and snapped values and does nothing when they are equal.
 *   2. Throttle. A drag produces up to ~125 mousemoves a second; a tap per event
 *      would be a buzz. `MIN_GAP_MS` is the floor between taps, and re-snapping
 *      to the SAME boundary is suppressed entirely — you crossed one edge, so you
 *      feel one click, however long you hover on it.
 *
 * Off macOS the Rust command is a no-op, so nothing here needs a platform check.
 */

/** Minimum time between taps. Below ~80 ms consecutive taps stop reading as
 *  separate events and start reading as vibration. */
const MIN_GAP_MS = 80

/** How close two snap targets must be to count as "the same one" (seconds).
 *  Prevents a re-tap while the pointer sits on a boundary and the snapped value
 *  is recomputed on every mousemove. */
const SAME_TARGET_SEC = 0.001

let lastAt = 0
let lastTarget = Number.NaN

function fire(pattern: 'alignment' | 'levelChange' | 'generic'): void {
  const now = Date.now()
  if (now - lastAt < MIN_GAP_MS) return
  lastAt = now
  void window.api.hapticPerform?.(pattern)
}

/**
 * A value was snapped to `snapped` from `raw`. Taps once — `alignment`, the
 * system pattern for "clicked into place" — when this is a NEW snap.
 *
 * Returns whether a tap was dispatched (for tests / callers that want to know).
 */
export function snapPulse(raw: number, snapped: number): boolean {
  if (raw === snapped) {
    // Not snapped — free movement. Arm the next real snap.
    lastTarget = Number.NaN
    return false
  }
  if (Math.abs(snapped - lastTarget) <= SAME_TARGET_SEC) return false
  lastTarget = snapped
  const before = lastAt
  fire('alignment')
  return lastAt !== before
}

/** A hard limit was hit — a trim handle pinned against the other edge, the
 *  playhead pushed out of a cut. `levelChange` is the system's detent pattern. */
export function limitPulse(): void {
  lastTarget = Number.NaN
  fire('levelChange')
}

/**
 * Reset between interactions, so the first snap of a new drag always taps.
 *
 * Clears the throttle as well as the last target: the throttle exists to stop a
 * single continuous drag from buzzing, and a fresh mousedown is by definition a
 * new intention — making the user wait 80 ms for the feel of their first snap
 * would be the throttle working against the thing it protects.
 */
export function resetHaptics(): void {
  lastTarget = Number.NaN
  lastAt = 0
}
