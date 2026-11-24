/**
 * Editor haptics — the trackpad clicks when a trim handle meets a hard limit.
 *
 * `haptic_perform` (commands/haptics.rs) maps logical patterns onto Apple's
 * `NSHapticFeedbackPattern`. A trim handle pinned against the other edge is the
 * moment a Mac is supposed to click under your finger.
 *
 * Throttle: a drag produces up to ~125 mousemoves a second; a tap per event
 * would be a buzz. `MIN_GAP_MS` is the floor between taps.
 *
 * Off macOS the Rust command is a no-op, so nothing here needs a platform check.
 */

/** Minimum time between taps. Below ~80 ms consecutive taps stop reading as
 *  separate events and start reading as vibration. */
const MIN_GAP_MS = 80;

let lastAt = 0;

function fire(pattern: "alignment" | "levelChange" | "generic"): void {
  const now = Date.now();
  if (now - lastAt < MIN_GAP_MS) return;
  lastAt = now;
  void window.api.hapticPerform?.(pattern);
}

/** A hard limit was hit — a trim handle pinned against the other edge, the
 *  playhead pushed out of a cut. `levelChange` is the system's detent pattern. */
export function limitPulse(): void {
  fire("levelChange");
}

/**
 * Reset between interactions, so the first tap of a new drag always fires.
 *
 * The throttle exists to stop a single continuous drag from buzzing, and a fresh
 * mousedown is by definition a new intention — making the user wait 80 ms for
 * the feel of their first tap would be the throttle working against the thing it
 * protects.
 */
export function resetHaptics(): void {
  lastAt = 0;
}
