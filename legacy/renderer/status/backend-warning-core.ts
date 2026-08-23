/**
 * Backend warnings → a localized toast.
 *
 * The `backend-warning` channel had a live consumer in pages/home.ts and, until
 * Fase 2 of "make it real", no emitter anywhere in src-tauri. Now that the
 * backend actually speaks on it (pre-roll gave up, crash recovery skipped a
 * file, the configured mixer is not plugged in, the disk is filling), the
 * renderer has to turn a machine code into a sentence in the user's language.
 *
 * The ordering is deliberate and is the whole design:
 *
 *   1. `code` → a `notify.*` locale key → the localized sentence,
 *   2. otherwise `msg`, the backend's own Norwegian wording,
 *   3. otherwise the bare code.
 *
 * Step 2 is what makes it safe for the backend to learn a new warning before
 * this table does: the user gets a true sentence in the wrong language rather
 * than silence. Silence is what this channel already produced for months.
 *
 * Pure — no DOM, no IPC, no i18n singleton (the localizer is injected, like
 * `health-findings.ts`), so every branch is unit-tested.
 */

import type { BackendWarning } from '../../bindings/BackendWarning'
import type { ToastKind } from '../ui/toast'

/** The localizer shape, injected so this module is testable in isolation. */
export type Translate = (key: string, fallback: string) => string

/** The count-aware localizer (`i18n.tn`). Injected for the same reason, and
 *  required rather than optional: a warning whose copy is governed by a count
 *  must never quietly fall back to the un-pluralized template. */
export type TranslateCount = (key: string, count: number, fallback: string) => string

/**
 * Stable backend code → locale key. Mirrors `sundayrec_core::notify::code`;
 * the Rust side's `code::ALL` is the authority and this table is checked
 * against it by the test below.
 */
export const WARNING_KEYS: Record<string, string> = {
  preroll_dead: 'notify.prerollDead',
  recovery_skipped: 'notify.recoverySkipped',
  device_missing: 'notify.deviceMissing',
  disk_low: 'notify.diskLow',
}

/**
 * Codes whose copy is governed by a count, and which param carries it.
 *
 * Empty today (the one count-governed code, the review queue's «har ventet i
 * {days} dager», left with the review queue). The mechanism stays: a locale
 * value under one of these keys is a plural GROUP, so the template has to be
 * picked before it is interpolated — wrong in Polish for 1 («1 dnia», not
 * «1 dni») and in every language for 1 day otherwise.
 */
export const WARNING_COUNT_PARAMS: Record<string, string> = {}

/** What home.ts needs to raise the toast. */
export interface WarningView {
  kind: ToastKind
  text: string
}

/**
 * Toast kind for a warning severity. `error` is sticky in the toast service —
 * which is right here: a mixer that is not plugged in before a scheduled
 * start is precisely the message that must not vanish while the operator
 * looks away.
 */
export function warningToastKind(severity: unknown): ToastKind {
  return severity === 'error' ? 'error' : 'warn'
}

/**
 * The interpolation values for a warning, with the derived ones added.
 *
 * `freeBytes` arrives as an exact byte count (the backend must not guess at the
 * user's unit conventions); `freeGb` is what a person can actually read, to one
 * decimal, using the same 1024³ GB the preflight card uses.
 */
export function warningParams(w: BackendWarning): Record<string, string> {
  const params: Record<string, string> = { ...(w.params ?? {}) }
  const free = Number(params.freeBytes)
  if (Number.isFinite(free) && free >= 0) {
    params.freeGb = (free / 1_073_741_824).toFixed(1)
  }
  return params
}

/** Replace every `{name}` for which we have a value. Unknown placeholders are
 *  left alone rather than blanked — a visible `{file}` is a bug report; a
 *  silently empty sentence is not. */
export function interpolate(template: string, params: Record<string, string>): string {
  return template.replace(/\{(\w+)\}/g, (whole, key: string) =>
    Object.prototype.hasOwnProperty.call(params, key) ? params[key] : whole,
  )
}

/**
 * Turn a `backend-warning` payload into a toast, or `null` when there is
 * nothing sayable (a malformed payload with neither a code nor a message).
 *
 * Tolerant of the payload shape on purpose: this crosses an IPC boundary, and a
 * warning that throws while telling you about a problem is worse than the
 * problem.
 */
export function toWarningView(
  data: unknown,
  t: Translate,
  tn: TranslateCount,
): WarningView | null {
  if (!data || typeof data !== 'object') return null
  const w = data as BackendWarning
  const code = typeof w.code === 'string' ? w.code : ''
  const msg = typeof w.msg === 'string' && w.msg ? w.msg : ''
  if (!code && !msg) return null

  const key = WARNING_KEYS[code]
  const params = warningParams(w)
  // `t`/`tn` return the fallback when the key is missing from the active
  // locale, so an untranslated key degrades to the backend's own wording, then
  // to the code.
  const countParam = WARNING_COUNT_PARAMS[code]
  const count = countParam === undefined ? NaN : Number(params[countParam])
  const template = !key
    ? msg || code
    : Number.isFinite(count)
      ? tn(key, count, msg || code)
      : t(key, msg || code)

  return {
    kind: warningToastKind(w.severity),
    text: interpolate(template, params),
  }
}
