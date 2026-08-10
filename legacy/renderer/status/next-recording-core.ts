/**
 * "Next recording" — the PURE model.
 *
 * Until now five places computed the same answer independently and could all
 * disagree at once:
 *
 *   1. the hero title           (counted settings.slots itself)
 *   2. the hero countdown       (froze the moment a recording started)
 *   3. the wake badge           (guessed "next − 10 min" client-side, while the
 *                                settings copy promised "~2 min")
 *   4. the sidebar status label (its own getNextRecording call + its own format,
 *                                overwritten again by the recording overlay)
 *   5. the Tidsplan preview     (hand-rolled weekday arithmetic that ignored
 *                                one-off specials entirely — a Christmas Eve
 *                                service simply did not exist to it)
 *
 * Every one of them now renders THIS state, which is fed by the scheduler
 * events the backend has been emitting all along. The formatters live here,
 * without a DOM, so the wording and the edge cases (nothing scheduled, no
 * schedule at all, a start that just passed, a take in progress) are covered by
 * the node vitest gate rather than by clicking around the app.
 *
 * `t` is injected rather than imported so tests can pass an identity function
 * and assert on structure instead of on Norwegian.
 */

import type { MissedRecordingInfo } from '../../bindings/MissedRecordingInfo'
import type { PreflightFinding } from '../../bindings/PreflightFinding'

/** i18n lookup, injected so the formatters stay pure. */
export type Translate = (key: string, fallback?: string) => string

/** Interpolating lookup (`i18n.tf`), injected for the same reason. */
export type TranslateF = (
  key: string,
  params: Record<string, string | number>,
  fallback?: string,
) => string

/** Count-aware lookup (`i18n.tn`), injected for the same reason. Picks the
 *  CLDR plural form for `count` in the active language — the thing `=== 1`
 *  could never do for Polish, where 2–4 takes its own noun form. */
export type TranslateN = (
  key: string,
  count: number,
  params?: Record<string, string | number>,
  fallback?: string,
) => string

/**
 * The little of a one-off special this module needs. Structural on purpose: the
 * renderer's `SpecialRecording` (optional `id`) and the ts-rs binding (nullable
 * `id`) are not assignable to each other, and matching a start time needs
 * neither.
 */
export interface SpecialLike {
  date: string
  name: string
  start: string
}

/**
 * How many minutes before a scheduled start the OS wake timer is armed.
 *
 * This is not a guess: it MIRRORS `WAKE_LEAD_MINUTES` in
 * `crates/sundayrec-core/src/wake.rs:29`, which is what actually schedules the
 * pmset/schtasks wake point. The Tidsplan copy used to promise "~2 min", the
 * home badge assumed 10 — the machine has always woken at 10.
 */
export const WAKE_LEAD_MINUTES = 10

/** The next scheduled start, as the scheduler sees it. */
export interface NextRecording {
  /** Zone-less local ISO exactly as the backend emits it: `YYYY-MM-DDTHH:MM:SS`. */
  at: string
  /** `at` parsed once, in epoch ms. */
  atMs: number
  /** The special recording's name, when this start comes from one. */
  label?: string
  /** True when a one-off special — not a weekly slot — produced this start. */
  isSpecial?: boolean
}

/** When (and whether) the machine wakes itself for `next`. */
export interface WakeInfo {
  /** The user's "vekk maskin automatisk" setting. */
  enabled: boolean
  /** Lead time actually used by the backend. */
  leadMinutes: number
  /** Epoch ms of the wake point (`next.atMs − leadMinutes`). */
  atMs: number
}

/** The single truth every "next recording" surface renders. */
export interface NextRecordingState {
  next: NextRecording | null
  /** A take is running right now (from the recorder's own state events). */
  isRecording: boolean
  /** Any weekly slot or one-off special exists at all — distinguishes "nothing
   *  scheduled ahead" from "this app has never been set up". */
  hasAnySchedule: boolean
  wake: WakeInfo | null
  /** Scheduled recordings that never ran (`scheduler://missed`). */
  missed: MissedRecordingInfo[]
  /** Findings from the pre-start check (`scheduler://preflight`). */
  preflight: PreflightFinding[]
}

export function emptyState(): NextRecordingState {
  return {
    next: null,
    isRecording: false,
    hasAnySchedule: false,
    wake: null,
    missed: [],
    preflight: [],
  }
}

// ── Parsing / deriving ───────────────────────────────────────────────────────

/**
 * Parse the backend's zone-less local ISO. A date-time string without a zone is
 * parsed as LOCAL time by every JS engine, which is exactly the frame Rust
 * produced it in (`scheduler::fmt_dt`).
 */
export function parseLocalIso(at: string): number {
  return new Date(at).getTime()
}

/** `YYYY-MM-DDTHH:MM` prefix — enough to match a special's date + start. */
function minutePrefix(at: string): string {
  return at.slice(0, 16)
}

/**
 * Turn the raw `scheduler://next` payload into a [`NextRecording`], naming it
 * when a one-off special produced it. The backend event carries only a
 * timestamp, but "Julaften-gudstjeneste 16:00" tells a volunteer far more than
 * "torsdag 16:00" — and the specials list is already in settings, so the name
 * costs nothing.
 */
export function buildNext(
  at: string | null | undefined,
  specials: SpecialLike[] = [],
): NextRecording | null {
  if (!at) return null
  const atMs = parseLocalIso(at)
  if (!Number.isFinite(atMs)) return null
  const key = minutePrefix(at)
  const special = specials.find(s => s && `${s.date}T${s.start}` === key)
  return special
    ? { at, atMs, label: special.name, isSpecial: true }
    : { at, atMs }
}

/** The wake point for `next`, or null when nothing is scheduled. */
export function computeWake(
  next: NextRecording | null,
  enabled: boolean,
  leadMinutes: number = WAKE_LEAD_MINUTES,
): WakeInfo | null {
  if (!next) return null
  return { enabled, leadMinutes, atMs: next.atMs - leadMinutes * 60_000 }
}

// ── Date rendering seam ──────────────────────────────────────────────────────

/** The four shapes of a start time the UI needs. */
export interface DateParts {
  /** "torsdag" */
  weekdayLong: string
  /** "tor." */
  weekdayShort: string
  /** "11:00" */
  time: string
  /** "torsdag 7. juni" */
  dateLong: string
}

/** Injected so the formatters are deterministic under test. */
export type DatePartsFn = (ms: number) => DateParts

/** The real, locale-aware implementation used by the app. */
export function intlParts(locale: string): DatePartsFn {
  return (ms: number): DateParts => {
    const d = new Date(ms)
    return {
      weekdayLong: d.toLocaleDateString(locale, { weekday: 'long' }),
      weekdayShort: d.toLocaleDateString(locale, { weekday: 'short' }),
      time: d.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' }),
      dateLong: d.toLocaleDateString(locale, { weekday: 'long', month: 'long', day: 'numeric' }),
    }
  }
}

/** Everything a formatter needs beyond the state itself. */
export interface FormatCtx {
  t: Translate
  tf: TranslateF
  tn: TranslateN
  parts: DatePartsFn
  /** `Date.now()` at render time — passed in so a tick is testable. */
  nowMs: number
}

/** How the countdown's duration ("2 d 3t") is rendered. Injected because the
 *  app's `fmtCountdown` reaches for `t()` internally. */
export type DurationFn = (ms: number) => string

// ── Formatters ───────────────────────────────────────────────────────────────

/** Hero title: "Alt er klart til torsdag 11:00". */
export function formatNextTitle(state: NextRecordingState, ctx: FormatCtx): string {
  const { t } = ctx
  if (!state.next) {
    return state.hasAnySchedule
      ? t('home.readyTitle', 'Alt er klart')
      : t('home.readyNoSchedule', 'Klar — sett opp en tidsplan for å starte automatisk')
  }
  const p = ctx.parts(state.next.atMs)
  return ctx.tf(
    'home.readyTitleDay',
    { day: p.weekdayLong, time: p.time },
    'Alt er klart til {day} {time}',
  )
}

/** Hero date line: "torsdag 7. juni" — with the special's name when it has one. */
export function formatNextDate(state: NextRecordingState, ctx: FormatCtx): string {
  if (!state.next) return '—'
  const p = ctx.parts(state.next.atMs)
  return state.next.label ? `${p.dateLong} · ${state.next.label}` : p.dateLong
}

/**
 * Hero countdown.
 *
 * Keeps counting DURING a recording — the old code returned early while
 * `__isRecording` was set, so the one moment you might want to know when the
 * next service starts (mid-take, deciding whether to stop) was the one moment
 * the number was frozen at whatever it happened to say.
 */
export function formatCountdown(
  state: NextRecordingState,
  ctx: FormatCtx,
  duration: DurationFn,
): string {
  if (!state.next) return ''
  const diff = state.next.atMs - ctx.nowMs
  if (diff <= 0) return ''
  const base = `${duration(diff)} ${ctx.t('home.untilStart', 'til oppstart')}`
  if (!state.isRecording) return base
  return `${ctx.t('status.recording', 'Tar opp nå')} · ${ctx.t('home.nextShort', 'neste')}: ${base}`
}

/** What the sidebar dot should look like. */
export type StatusDot = '' | 'recording' | 'warn'

export interface SidebarStatus {
  text: string
  dot: StatusDot
}

export interface DeviceStatus {
  connected: boolean
  name?: string | null
}

/**
 * Sidebar status label + dot. The recording overlay used to write this element
 * directly on show/hide, racing the home page's own async computation; now
 * every writer goes through this one function.
 */
export function formatSidebarStatus(
  state: NextRecordingState,
  ctx: FormatCtx,
  device: DeviceStatus = { connected: true },
): SidebarStatus {
  if (state.isRecording) {
    return { text: ctx.t('status.recording', 'Tar opp nå'), dot: 'recording' }
  }
  if (!device.connected) {
    const name = device.name ? `: ${device.name}` : ''
    return { text: ctx.t('status.warning', 'Trenger oppmerksomhet') + name, dot: 'warn' }
  }
  if (!state.next) {
    return { text: ctx.t('status.noSchedule', 'Ingen opptak planlagt'), dot: '' }
  }
  const p = ctx.parts(state.next.atMs)
  return { text: `${p.weekdayShort} ${p.time}`, dot: '' }
}

/**
 * The wake badge: "Maskinen vekkes automatisk kl. 10:50 (10 min før)".
 *
 * Returns null when there is nothing to say — no schedule, or the user turned
 * wake-from-sleep off. The lead time is the backend's, not a UI guess.
 */
export function formatWakeHint(state: NextRecordingState, ctx: FormatCtx): string | null {
  const wake = state.wake
  if (!wake || !wake.enabled || !state.next) return null
  const time = ctx.parts(wake.atMs).time
  return ctx.tf('home.wakesBefore', { time }, 'Maskinen vekkes automatisk kl. {time}')
    + ' '
    + ctx.tf('home.wakesLead', { min: wake.leadMinutes }, '({min} min før)')
}

/**
 * Tidsplan preview: "Neste opptak: torsdag 7. juni kl. 11:00".
 *
 * Same source as the hero, so a special recording now shows up here too — the
 * old hand-rolled weekday loop only ever looked at weekly slots.
 */
export function formatSchedulePreview(state: NextRecordingState, ctx: FormatCtx): string {
  if (!state.next) return ''
  const p = ctx.parts(state.next.atMs)
  const when = `${p.dateLong} ${ctx.t('schedule.atTime', 'kl.')} ${p.time}`
  const line = `${ctx.t('schedule.nextPreviewPrefix', 'Neste opptak')}: ${when}`
  return state.next.label ? `${line} — ${state.next.label}` : line
}

/** Missed-recording summary line: "Søndag 11:00 ble ikke tatt opp". */
export function formatMissed(missed: MissedRecordingInfo, ctx: FormatCtx): string {
  const ms = parseLocalIso(missed.at)
  if (!Number.isFinite(ms)) return missed.label
  const p = ctx.parts(ms)
  return `${p.dateLong} ${ctx.t('schedule.atTime', 'kl.')} ${p.time} — ${missed.label}`
}

/** Banner headline for one or more missed recordings. */
export function formatMissedBanner(state: NextRecordingState, ctx: FormatCtx): string | null {
  const n = state.missed.length
  if (n === 0) return null
  return ctx.tn('missed.banner', n, {}, '{n} planlagte opptak ble ikke tatt opp')
}

/** Preflight headline: errors dominate warnings. */
export function formatPreflightHeadline(
  findings: PreflightFinding[],
  ctx: FormatCtx,
): { text: string; severity: 'warn' | 'error' } | null {
  if (findings.length === 0) return null
  const errors = findings.filter(f => f.severity === 'error').length
  const warns = findings.length - errors
  if (errors > 0) {
    return {
      severity: 'error',
      text: ctx.tn('status.preflightErrors', errors, {}, '{n} feil må rettes før opptaket'),
    }
  }
  return {
    severity: 'warn',
    text: ctx.tn('status.preflightWarns', warns, {}, '{n} ting å se på før opptaket'),
  }
}
