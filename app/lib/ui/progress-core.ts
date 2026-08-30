/**
 * progress-core — «hvor lenge er det igjen?», answered honestly.
 *
 * Every long job in this app (a two-hour service being decoded, transcribed,
 * analysed, mastered, exported) has until now told the user one of two things:
 * a percentage with no sense of time, or an indeterminate stripe that says only
 * "something is happening". Neither answers the question the operator actually
 * has on a Sunday afternoon — can I go make coffee, or is this thirty seconds?
 *
 * This module is the arithmetic behind that answer, kept pure so it can be
 * table-tested: feed it (fraction, now) samples, read back a remaining-time
 * estimate and whether that estimate is worth showing yet. The DOM lives in
 * `progress.ts`.
 *
 * Four rules, each of which exists because the naive version is worse:
 *
 *  1. RATE, not average. A linear extrapolation from the start ("we did 10 % in
 *     20 s, so 100 % takes 200 s") cannot recover from a phase change, and our
 *     exports have exactly that: a slow loudness measure pass followed by a fast
 *     encode. The rate is an EMA over instantaneous rates with a time-based
 *     alpha (the same `alphaFor` law the VU meters use), so a phase change
 *     re-converges within a few seconds instead of poisoning the whole run.
 *
 *  2. A WARM-UP GATE. The first sample carries no rate at all and the second
 *     carries a wildly noisy one. Until we have three samples spanning two
 *     seconds we say «beregner …» rather than «ca. 4 t igjen».
 *
 *  3. MONOTONE DISPLAY. An estimate that ticks UPWARD is read as a broken
 *     progress bar even when it is the truthful thing to say. Small upward
 *     movement is therefore held (the display just coasts down), and only a
 *     genuine slowdown — more than {@link GROW_TOLERANCE}× the coasted value —
 *     is allowed through, eased rather than snapped.
 *
 *  4. COARSE BUCKETS. «ca. 2 min igjen» is right often enough to be useful;
 *     «1 min 47 s igjen» is wrong in a way the user can see. {@link etaBucket}
 *     quantises before anything reaches the screen, which is also what makes
 *     rule 3's held jitter invisible.
 *
 * A stall (rate → 0) first grows the estimate gracefully through rule 3, then —
 * after {@link STALL_MS} with no movement at all — drops back to «beregner …»,
 * because at that point we genuinely do not know.
 */

import { alphaFor } from "../audio/smoothing";

/** Samples that MOVED (fraction increased) before an estimate is offered. */
export const MIN_SAMPLES = 3;
/** Wall time the samples must span before an estimate is offered, ms. */
export const MIN_SPAN_MS = 2000;
/** τ for the rate EMA, ms. ~4 s: a phase change re-converges in about that. */
export const RATE_TAU_MS = 4000;
/** No forward movement for this long ⇒ we admit we don't know any more, ms. */
export const STALL_MS = 5000;
/** How much worse than the coasted value a raw estimate must be before the
 *  display is allowed to grow at all. Below this it is noise, and held. */
export const GROW_TOLERANCE = 1.25;
/** Fraction of an accepted growth applied per update — grow eased, not snapped. */
export const GROW_EASE = 0.25;
/** Sanity clamp so a near-zero rate yields a big number rather than Infinity. */
export const MAX_ETA_MS = 24 * 3600_000;
/**
 * How far backward a fraction may drift before we call it a restarted phase
 * rather than noise. Producers accumulate their fraction in floating point
 * (`bytesRead / expected`, `currentSec / totalSec`), so an exactly-repeated
 * value routinely arrives an ulp BELOW the previous one — re-warming on that
 * would drop the estimate to «beregner …» at random moments.
 */
export const BACKWARD_EPS = 0.005;

/** What the caller should paint. `etaMs === null` means «beregner …». */
export interface EtaReading {
  /** Estimated milliseconds remaining, or `null` when we don't trust one yet. */
  etaMs: number | null;
  /** Whether {@link etaMs} is worth showing (false while warming up/stalled). */
  stable: boolean;
}

export interface EtaEstimator {
  /** Feed one observation. `fraction` is 0..1, `nowMs` a monotone clock. */
  push(fraction: number, nowMs: number): EtaReading;
  /** Re-read at `nowMs` with no new observation — drives the count-down between
   *  events and the stall detection. Never invents forward progress. */
  read(nowMs: number): EtaReading;
  /** Snap to "done": 0 ms remaining, stable. */
  complete(): EtaReading;
  /** Forget everything (a new job on the same widget). */
  reset(): void;
}

const UNKNOWN: EtaReading = { etaMs: null, stable: false };

/**
 * A remaining-time estimator for one job.
 *
 * `nowMs` is the caller's clock (`performance.now()` in the renderer, a plain
 * number in tests) — the estimator only ever takes differences, so any
 * monotone millisecond source works.
 */
export function createEtaEstimator(): EtaEstimator {
  let firstMs = 0;
  let lastMs = 0;
  let lastFraction = 0;
  let started = false;
  let moved = 0;
  let rate = 0;
  let rateInit = false;
  let lastMoveMs = 0;
  let displayed: number | null = null;
  let lastDisplayMs = 0;
  let finished = false;

  function reset(): void {
    started = false;
    finished = false;
    moved = 0;
    rate = 0;
    rateInit = false;
    displayed = null;
  }

  /** Restart the warm-up window without losing the fact that we have a fraction.
   *  Used when progress jumps BACKWARD (a phase restarted): the rate we learned
   *  describes work that is being redone, so it is not evidence about the rest. */
  function rewarm(nowMs: number, fraction: number): void {
    firstMs = nowMs;
    lastMs = nowMs;
    lastFraction = fraction;
    lastMoveMs = nowMs;
    moved = 0;
    rate = 0;
    rateInit = false;
    displayed = null;
  }

  function advance(nowMs: number): EtaReading {
    if (finished) return { etaMs: 0, stable: true };
    if (!started) return UNKNOWN;
    // Completion snap: the last thing the bar says before it disappears must be
    // "done", never a leftover «ca. 20 s igjen» under a full bar.
    if (lastFraction >= 1) {
      displayed = 0;
      lastDisplayMs = nowMs;
      return { etaMs: 0, stable: true };
    }

    const stalled = nowMs - lastMoveMs >= STALL_MS;
    const warm =
      moved >= MIN_SAMPLES &&
      nowMs - firstMs >= MIN_SPAN_MS &&
      rateInit &&
      rate > 0;
    if (!warm || stalled) {
      // Drop the held display: whatever resumes should adopt the truth of the
      // moment rather than easing away from a number we no longer believe.
      displayed = null;
      return UNKNOWN;
    }

    const raw = Math.min(MAX_ETA_MS, (1 - lastFraction) / rate);
    if (displayed === null) {
      displayed = raw;
    } else {
      // What the display would read if we simply let the clock run.
      const coasted = Math.max(0, displayed - (nowMs - lastDisplayMs));
      if (raw <= coasted) displayed = raw;
      else if (raw > coasted * GROW_TOLERANCE)
        displayed = coasted + (raw - coasted) * GROW_EASE;
      else displayed = coasted;
    }
    lastDisplayMs = nowMs;
    return { etaMs: displayed, stable: true };
  }

  return {
    push(fraction: number, nowMs: number): EtaReading {
      if (!Number.isFinite(fraction) || !Number.isFinite(nowMs))
        return advance(lastMs);
      const f = Math.max(0, Math.min(1, fraction));
      if (!started) {
        started = true;
        firstMs = nowMs;
        lastMs = nowMs;
        lastFraction = f;
        lastMoveMs = nowMs;
        lastDisplayMs = nowMs;
        return advance(nowMs);
      }
      if (f < lastFraction - BACKWARD_EPS) {
        rewarm(nowMs, f);
        return advance(nowMs);
      }
      // Sub-epsilon backward drift is float noise, not a rewind — floor it and
      // let it count as "no movement in this interval", which it is.
      const observed = Math.max(f, lastFraction);
      const dt = nowMs - lastMs;
      if (dt <= 0) {
        // Same-millisecond burst: take the higher fraction, learn no rate from
        // a zero-length interval (that division is where Infinity comes from).
        if (observed > lastFraction) lastMoveMs = nowMs;
        lastFraction = observed;
        return advance(nowMs);
      }
      const df = observed - lastFraction;
      const instRate = df / dt;
      if (df > 0) {
        moved++;
        lastMoveMs = nowMs;
        if (!rateInit) {
          rate = instRate;
          rateInit = true;
        } else {
          rate += (instRate - rate) * alphaFor(dt, RATE_TAU_MS);
        }
      } else if (rateInit) {
        // No movement in this interval is evidence too — let the rate decay so
        // the estimate grows instead of freezing at a rate we no longer see.
        rate += (0 - rate) * alphaFor(dt, RATE_TAU_MS);
      }
      lastFraction = observed;
      lastMs = nowMs;
      return advance(nowMs);
    },

    read(nowMs: number): EtaReading {
      if (!Number.isFinite(nowMs)) return advance(lastMs);
      return advance(nowMs);
    },

    complete(): EtaReading {
      finished = true;
      displayed = 0;
      return { etaMs: 0, stable: true };
    },

    reset,
  };
}

// ── Formatting ───────────────────────────────────────────────────────────────

/**
 * The quantised shape of an estimate. Deliberately coarse: see rule 4 in the
 * module header. `unknown` is «beregner …».
 */
export type EtaBucket =
  | { kind: "unknown" }
  | { kind: "under10s" }
  /** Whole tens of seconds, 10–50. */
  | { kind: "seconds"; value: number }
  /** Whole minutes, 1–59 (rounded to 5 above ten minutes). */
  | { kind: "minutes"; value: number }
  /** Hours plus whole tens of minutes, 0–50. */
  | { kind: "hours"; hours: number; minutes: number };

/** Quantise a raw millisecond estimate into what we are willing to claim. */
export function etaBucket(ms: number | null): EtaBucket {
  if (ms === null || !Number.isFinite(ms)) return { kind: "unknown" };
  if (ms < 10_000) return { kind: "under10s" };
  if (ms < 55_000) {
    const tens = Math.round(ms / 10_000) * 10;
    return { kind: "seconds", value: Math.max(10, Math.min(50, tens)) };
  }
  if (ms < 90_000) return { kind: "minutes", value: 1 };
  const minutes = ms / 60_000;
  if (minutes < 10)
    return { kind: "minutes", value: Math.max(1, Math.round(minutes)) };
  if (minutes < 60) {
    const fives = Math.round(minutes / 5) * 5;
    return { kind: "minutes", value: Math.max(10, Math.min(55, fives)) };
  }
  let hours = Math.floor(ms / 3600_000);
  let rest = Math.round((ms - hours * 3600_000) / 600_000) * 10;
  if (rest >= 60) {
    hours += 1;
    rest = 0;
  }
  return { kind: "hours", hours, minutes: rest };
}

/** The translator shape this module needs — `i18n.t`, without importing it. */
export type TranslateFn = (key: string, fallback?: string) => string;

/** Interpolating localizer (`i18n.tf`), injected for the same reason as `t`. */
export type TranslateFFn = (
  key: string,
  params: Record<string, string | number>,
  fallback?: string,
) => string;

/**
 * Render an estimate as the one line we put under a progress bar.
 * `null` (or an unstable reading) renders «beregner …», never a guess.
 */
export function formatEta(
  ms: number | null,
  t: TranslateFn,
  tf: TranslateFFn,
): string {
  const b = etaBucket(ms);
  switch (b.kind) {
    case "unknown":
      return t("progress.etaCalculating", "beregner …");
    case "under10s":
      return t("progress.etaUnder10s", "under 10 s igjen");
    case "seconds":
      return tf("progress.etaSeconds", { n: b.value }, "ca. {n} s igjen");
    case "minutes":
      return tf("progress.etaMinutes", { n: b.value }, "ca. {n} min igjen");
    case "hours":
      return b.minutes === 0
        ? tf("progress.etaHours", { h: b.hours }, "ca. {h} t igjen")
        : tf(
            "progress.etaHoursMinutes",
            { h: b.hours, m: b.minutes },
            "ca. {h} t {m} min igjen",
          );
  }
}

/** `0.4271` → `43` — the only rounding a percentage label ever needs. */
export function formatPercent(fraction: number): number {
  if (!Number.isFinite(fraction)) return 0;
  return Math.round(Math.max(0, Math.min(1, fraction)) * 100);
}
