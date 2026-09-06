/**
 * Time-based level smoothing — ONE law of motion for every meter in the app.
 *
 * Every meter used to invent its own: the home VU eased with a per-FRAME
 * constant (`sm*0.55 + raw*0.45`), the channel grid leaned on a CSS
 * `transition: transform .05s` while a rAF writer fought it, and only the
 * recording overlay got the dt-based treatment in v0.5.0. A per-frame constant
 * defines the motion in FRAMES, so the needle's law visibly changes the moment
 * the frame rate dips — a jank amplifier, because the eye reads the change in
 * behaviour as a stutter on top of the dropped frame itself.
 *
 * `1 - exp(-dt/τ)` fixes that: the fraction of the remaining distance covered
 * depends on elapsed TIME, so 60 → 30 fps changes how often the meter is
 * redrawn and nothing else about how it moves.
 *
 * Attack and release are separate τ because a meter must be honest upward and
 * calm downward: a peak that arrives late is a lie, a peak that leaves slowly
 * is legible. The default attack τ = 0 means "snap up instantly" — the same
 * rise-instant/fall-eased shape the recording overlay has been shipping since
 * v0.5.0 (see easeFallAlpha in pages/recording.ts, which now delegates here).
 *
 * Pure, DOM-free, unit-tested (smoothing.test.ts) for frame-rate independence.
 */

/** Rise instantly by default — a level meter that lags upward under-reports. */
export const ATTACK_TAU_MS = 0;
/** τ ≈ 80 ms release: ~184 ms to cover 90 % of a fall. Hardware-tuned on the
 *  recording overlay; every other meter now inherits the same feel. */
export const RELEASE_TAU_MS = 80;
/** Longest dt a single step may act on. A tab that was backgrounded for two
 *  seconds must not teleport the meter — it resumes, it doesn't replay. */
export const MAX_STEP_MS = 200;

/**
 * Fraction of the remaining distance covered in `dtMs` for an exponential
 * approach with time constant `tauMs`.
 *
 * The property that matters: `alphaFor` composes. Two steps of dt/2 leave the
 * same remaining distance as one step of dt, because
 * (1−α(dt/2))·(1−α(dt/2)) = exp(−dt/2τ)·exp(−dt/2τ) = exp(−dt/τ) = 1−α(dt).
 * That identity IS frame-rate independence.
 *
 * τ ≤ 0 means no smoothing at all (snap); dt ≤ 0 means no time passed.
 */
export function alphaFor(dtMs: number, tauMs: number): number {
  if (tauMs <= 0) return 1;
  if (dtMs <= 0) return 0;
  return 1 - Math.exp(-dtMs / tauMs);
}

export interface LevelSmootherOpts {
  /** τ for upward movement, ms. 0 = instant (default). */
  attackTau?: number;
  /** τ for downward movement, ms. Default 80. */
  releaseTau?: number;
  /** Starting (and `reset()`) value. Default −60, the dBFS floor. */
  initial?: number;
}

export interface LevelSmoother {
  /** Advance toward `target` by `dtMs` of real time; returns the new value. */
  step(target: number, dtMs: number): number;
  /** The current smoothed value, without advancing. */
  readonly value: number;
  /** Jump back to `to` (default: the configured initial value). */
  reset(to?: number): void;
}

/**
 * A one-pole smoother with separate attack/release time constants.
 *
 * Units are the caller's — dBFS for the VU meters, 0..1 fractions for the
 * channel grid. Only `dtMs` has a fixed meaning.
 */
export function createLevelSmoother(
  opts: LevelSmootherOpts = {},
): LevelSmoother {
  const attackTau = opts.attackTau ?? ATTACK_TAU_MS;
  const releaseTau = opts.releaseTau ?? RELEASE_TAU_MS;
  const initial = opts.initial ?? -60;
  let value = initial;

  return {
    get value(): number {
      return value;
    },
    reset(to: number = initial): void {
      value = to;
    },
    step(target: number, dtMs: number): number {
      // A NaN target (a dropped telemetry field, a divide-by-zero upstream)
      // would poison the state forever — one bad packet, a dead meter.
      if (!Number.isFinite(target)) return value;
      const dt = Math.min(MAX_STEP_MS, Math.max(0, dtMs));
      value +=
        (target - value) *
        alphaFor(dt, target > value ? attackTau : releaseTau);
      return value;
    },
  };
}
