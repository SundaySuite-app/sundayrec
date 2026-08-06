/**
 * progress — one progress bar for the whole app.
 *
 * Before this there were four: the transcribe modal's, the export row's, the
 * mastering row's and the home health check's. Four bars, four ways of saying
 * "unknown", four different animations, and not one of them ever said how long
 * was left. They also all moved the same way — `style.width` plus a CSS
 * `transition: width`, which is a layout-and-paint per tick and visibly
 * stair-steps when ticks arrive in bursts (the v0.9.0 finding). This renders the
 * fill with `transform: scaleX()` written from a single rAF, so a burst of ten
 * events costs one composited frame.
 *
 * The widget owns an {@link createEtaEstimator} and does the honest thing with
 * it: percentage from the caller, remaining time from the estimator, «beregner …»
 * whenever the estimator is not confident. Callers just feed fractions.
 *
 *     const p = attachProgress(host, { label: 'Transkriberer …' })
 *     p.update(0.42)          // 42 % + an ETA line once it settles
 *     p.update(null)          // indeterminate — we genuinely don't know
 *     p.done()                // snap to full, ETA line clears
 *
 * `update(null)` is not a failure mode to be ashamed of: several backends
 * really cannot report a denominator, and a sliding stripe is the truth there.
 * What it must never be is a bar frozen at 0 %.
 */

import { t } from '../i18n'
import { prefersReducedMotion } from './motion'
import { createEtaEstimator, formatEta, formatPercent, type EtaEstimator } from './progress-core'

export interface ProgressOpts {
  /** Leading text, e.g. «Analyserer bølgeform …». Changeable per update. */
  label?: string
  /** Show the remaining-time line. Default true. */
  eta?: boolean
  /** How often the ETA line refreshes between events, ms. Default 1000. */
  tickMs?: number
  /** Inline variant for a table row / button area (thinner, one line). */
  compact?: boolean
}

export interface ProgressHandle {
  /** `fraction` 0..1, or `null` for "running, denominator unknown". */
  update(fraction: number | null, label?: string): void
  /** Snap to 100 %, clear the ETA, stop the ticker. */
  done(label?: string): void
  /** Mark the bar failed (red), stop the ticker. */
  fail(label?: string): void
  /** Stop timers and empty the host. */
  destroy(): void
  /** The widget root, for callers that want to show/hide it. */
  readonly el: HTMLElement
}

/** One handle per host: re-attaching replaces cleanly instead of stacking. */
const attached = new WeakMap<HTMLElement, ProgressHandle>()

/** rAF where it exists, a 16 ms timer where it does not (node, old webviews). */
function raf(cb: () => void): number {
  return typeof requestAnimationFrame === 'function'
    ? requestAnimationFrame(() => cb())
    : (setTimeout(cb, 16) as unknown as number)
}
function cancelRaf(id: number): void {
  if (typeof cancelAnimationFrame === 'function') cancelAnimationFrame(id)
  else clearTimeout(id)
}

function el(tag: string, cls: string): HTMLElement {
  const n = document.createElement(tag)
  n.className = cls
  return n
}

/**
 * Build a progress widget inside `host` (whose previous contents are replaced).
 * The host itself keeps whatever layout the surrounding page gave it — the
 * widget is `display:block` and fills the width it is given.
 */
export function attachProgress(host: HTMLElement, opts: ProgressOpts = {}): ProgressHandle {
  attached.get(host)?.destroy()

  const showEta = opts.eta !== false
  const tickMs = opts.tickMs ?? 1000

  const root = el('div', 'progress' + (opts.compact ? ' progress--compact' : ''))
  root.setAttribute('role', 'progressbar')
  root.setAttribute('aria-valuemin', '0')
  root.setAttribute('aria-valuemax', '100')
  if (prefersReducedMotion()) root.classList.add('is-reduced')

  const track = el('div', 'progress-track')
  const fill = el('div', 'progress-fill')
  track.appendChild(fill)

  const meta = el('div', 'progress-meta')
  const labelEl = el('span', 'progress-label')
  const status = el('span', 'progress-status')
  const pctEl = el('span', 'progress-pct')
  const etaEl = el('span', 'progress-eta')
  status.append(pctEl, etaEl)
  meta.append(labelEl, status)

  root.append(track, meta)
  host.textContent = ''
  host.appendChild(root)

  const est: EtaEstimator = createEtaEstimator()
  let label = opts.label ?? ''
  /** null = indeterminate. Latched so the ticker knows what to redraw. */
  let fraction: number | null = null
  /** True once a real number has arrived — before that the ETA line stays out
   *  of the way rather than promising an estimate for an unmeasured job. */
  let everNumeric = false
  let finished = false
  let rafId = 0
  let timer = 0

  /** All DOM writes for a frame, in one place: one style write, three texts. */
  function paint(): void {
    rafId = 0
    if (fraction === null) {
      root.classList.add('is-indeterminate')
      root.removeAttribute('aria-valuenow')
      fill.style.transform = ''
      pctEl.textContent = ''
    } else {
      root.classList.remove('is-indeterminate')
      const clamped = Math.max(0, Math.min(1, fraction))
      fill.style.transform = `scaleX(${clamped.toFixed(4)})`
      const pct = formatPercent(clamped)
      root.setAttribute('aria-valuenow', String(pct))
      pctEl.textContent = `${pct} %`
    }
    labelEl.textContent = label
    etaEl.textContent = etaText()
  }

  function etaText(): string {
    if (!showEta || finished || !everNumeric) return ''
    const reading = est.read(performance.now())
    return formatEta(reading.stable ? reading.etaMs : null, t)
  }

  function schedule(): void {
    if (rafId) return
    rafId = raf(paint)
  }

  /** The ETA line has to keep counting down between events — a transcription
   *  emits every few seconds, and a frozen «ca. 2 min igjen» reads as a hang. */
  function startTicker(): void {
    if (timer || !showEta) return
    timer = window.setInterval(() => {
      if (finished) return
      etaEl.textContent = etaText()
    }, tickMs)
  }

  function stopTicker(): void {
    if (timer) {
      clearInterval(timer)
      timer = 0
    }
  }

  const handle: ProgressHandle = {
    el: root,

    update(next: number | null, nextLabel?: string): void {
      if (finished) return
      if (nextLabel !== undefined) label = nextLabel
      if (next === null || !Number.isFinite(next)) {
        fraction = null
      } else {
        fraction = Math.max(0, Math.min(1, next))
        everNumeric = true
        est.push(fraction, performance.now())
        startTicker()
      }
      schedule()
    },

    done(nextLabel?: string): void {
      finished = true
      stopTicker()
      if (nextLabel !== undefined) label = nextLabel
      fraction = 1
      est.complete()
      schedule()
    },

    fail(nextLabel?: string): void {
      finished = true
      stopTicker()
      if (nextLabel !== undefined) label = nextLabel
      root.classList.add('is-failed')
      // Leave the bar where it stopped: how far it got is information.
      if (fraction === null) fraction = 0
      schedule()
    },

    destroy(): void {
      stopTicker()
      if (rafId) {
        cancelRaf(rafId)
        rafId = 0
      }
      // Only relinquish the host if we still OWN it. A second `attachProgress`
      // on the same element (two overlapping runs — the editor's automatic
      // analysis and a user clicking «Analyser opptak») replaces us, and the
      // loser's teardown must not then delete the winner's registration.
      if (attached.get(host) === handle) attached.delete(host)
      if (root.parentNode === host) host.removeChild(root)
    },
  }

  attached.set(host, handle)
  paint()
  return handle
}
