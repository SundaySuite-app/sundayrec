/**
 * bind-setting-core — the decisions behind `bindSetting`, with no DOM in them.
 *
 * Settings pages used to carry three save models at once: audio/video saved
 * silently on change, Filer/Publisering/System had a dirty-footer with
 * Lagre/Avbryt (where «Avbryt» could not actually revert anything, because half
 * the controls had already written), and Sunday-suite had three inline «Lagre»
 * buttons. A volunteer operator had no way to know which surface they were on.
 *
 * One model now: everything auto-applies, with a visible receipt. The parts
 * worth testing are the parts that decide WHEN a change commits and WHAT value
 * it commits — so they live here, pure, instead of inside an event handler.
 */

/** The control shapes the settings pages actually use. */
export type ControlKind =
  | 'toggle'
  | 'radio'
  | 'select'
  | 'slider'
  | 'number'
  | 'text'
  | 'textarea'

/** Everything a settings control can hold once coerced. */
export type SettingValue = string | number | boolean | null

/**
 * Trailing debounce for free typing. Long enough that a name typed at speed is
 * one write rather than fifteen, short enough that the «Lagret ✓» chip still
 * feels like a response to what you just did.
 */
export const TEXT_DEBOUNCE_MS = 500

/**
 * Window handed to `saveSettingsDebounced` once a change has been committed.
 * The commit debounce above already collapsed the typing; this only coalesces
 * several controls changed in the same breath (a toggle that reveals a select
 * the user immediately sets) into one IPC round-trip.
 */
export const SAVE_COALESCE_MS = 120

/** How long the inline «Lagret ✓» chip stays up — matches the channel grid. */
export const SAVED_CHIP_MS = 1600

/** The minimal description of a control needed to classify it. */
export interface ControlShape {
  /** Lower- or upper-case tag name; case is not significant. */
  tag: string
  /** `input.type`, when the tag is an input. */
  type?: string | null
}

/**
 * Classify a control from its tag + type. `radio` is reported for radio inputs
 * even though they are normally bound as a group — the group binder passes the
 * same kind through, so the commit plan is identical either way.
 */
export function controlKindOf(shape: ControlShape): ControlKind {
  const tag = (shape.tag || '').toLowerCase()
  if (tag === 'select') return 'select'
  if (tag === 'textarea') return 'textarea'
  const type = (shape.type || 'text').toLowerCase()
  if (type === 'checkbox') return 'toggle'
  if (type === 'radio') return 'radio'
  if (type === 'range') return 'slider'
  if (type === 'number') return 'number'
  return 'text'
}

/** When a change commits, and which DOM events can trigger it. */
export interface CommitPlan {
  /** 0 = commit on the event itself. */
  debounceMs: number
  events: string[]
}

export interface CommitOverride {
  /** Force an immediate commit even for a text field. */
  immediate?: boolean
  /** Explicit debounce, wins over the kind's default. */
  debounceMs?: number
}

/**
 * Decide when a control's change becomes a write.
 *
 * Discrete controls (toggle, radio, select) commit on `change`, which is the
 * event that means "the user decided". A range input also fires `change` on
 * release only — dragging fires `input` — so a slider commits on release and
 * never writes 40 times across one gesture. Free text debounces.
 */
export function planCommit(kind: ControlKind, override: CommitOverride = {}): CommitPlan {
  const discrete = kind === 'toggle' || kind === 'radio' || kind === 'select' || kind === 'slider'
  const defaultMs = discrete ? 0 : TEXT_DEBOUNCE_MS
  const debounceMs = override.immediate
    ? 0
    : override.debounceMs !== undefined
      ? Math.max(0, override.debounceMs)
      : defaultMs
  // A debounced control must listen on `input` too, or a value typed and left
  // alone (no blur, no Enter) would never commit at all.
  const events = discrete ? ['change'] : ['input', 'change']
  return { debounceMs, events }
}

/** The raw DOM state a control exposes, without importing the DOM. */
export interface RawControlValue {
  value: string
  checked?: boolean
}

/**
 * Turn a control's raw state into the value that belongs in Settings.
 *
 * Numeric controls return `null` (not `NaN`) for an unparseable value, so the
 * caller can tell "the user cleared the field" from "the user typed 0" — the
 * distinction the old `parseInt(x) || 0` idiom threw away.
 */
export function coerceValue(kind: ControlKind, raw: RawControlValue): SettingValue {
  if (kind === 'toggle') return !!raw.checked
  if (kind === 'number' || kind === 'slider') {
    const trimmed = (raw.value ?? '').trim()
    if (trimmed === '') return null
    const n = Number(trimmed)
    return Number.isFinite(n) ? n : null
  }
  return raw.value ?? ''
}

/** Why a numeric value was rejected — translated by the DOM layer. */
export type NumberIssue = 'nan' | 'below' | 'above' | 'notInteger'

export interface NumberRule {
  min?: number
  max?: number
  /** Reject non-integers instead of rounding them. */
  integer?: boolean
  /** Clamp instead of rejecting — used where a hard bound is safe to apply. */
  clamp?: boolean
}

export interface NumberCheck {
  ok: boolean
  /** The value to use when `ok` — clamped when the rule asks for clamping. */
  value: number | null
  issue?: NumberIssue
  /** The bound that was hit, for a message like "minimum er 500". */
  bound?: number
}

/**
 * Validate (and optionally clamp) a numeric setting.
 *
 * Kept separate from `coerceValue` because rejection and coercion are different
 * decisions: the bitrate field clamps (any number is safe, out-of-range ones
 * just aren't), while «slett etter N dager» must reject rather than quietly
 * pick a number that deletes recordings on a schedule nobody asked for.
 */
export function validateNumber(value: SettingValue, rule: NumberRule = {}): NumberCheck {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return { ok: false, value: null, issue: 'nan' }
  }
  if (rule.integer && !Number.isInteger(value)) {
    return { ok: false, value: null, issue: 'notInteger' }
  }
  if (rule.min !== undefined && value < rule.min) {
    return rule.clamp
      ? { ok: true, value: rule.min, bound: rule.min }
      : { ok: false, value: null, issue: 'below', bound: rule.min }
  }
  if (rule.max !== undefined && value > rule.max) {
    return rule.clamp
      ? { ok: true, value: rule.max, bound: rule.max }
      : { ok: false, value: null, issue: 'above', bound: rule.max }
  }
  return { ok: true, value }
}

/**
 * Whether a change is worth committing at all. A `change` event fires on a
 * radio group when the SAME option is re-clicked in some browsers, and a text
 * field re-fires `change` on blur with no edit — writing settings and flashing
 * «Lagret ✓» for a non-change is noise that teaches the user to ignore the chip.
 */
export function isRealChange(previous: SettingValue, next: SettingValue): boolean {
  return previous !== next
}

/** How urgently the app should ask before applying a change. */
export type GuardReason = 'recording' | 'imminent' | null

/**
 * Decide whether a settings change needs a confirmation first, given how close
 * the next recording is. Changing the audio device 4 minutes before the service
 * starts is the change that silently costs you the recording, so it gets a
 * question — but only then, so the guard never becomes background noise.
 *
 * `leadMinutes` defaults to the backend's wake lead (10), which is also the
 * window in which the machine may already be waking up for the slot.
 */
export function guardReasonFor(
  input: { isRecording: boolean; nextAtMs: number | null },
  nowMs: number,
  leadMinutes = 10,
): GuardReason {
  if (input.isRecording) return 'recording'
  if (input.nextAtMs === null) return null
  const deltaMs = input.nextAtMs - nowMs
  if (deltaMs < 0) return null
  return deltaMs <= leadMinutes * 60_000 ? 'imminent' : null
}

/** Whole minutes until the next start, floored, for the guard's wording. */
export function minutesUntil(nextAtMs: number, nowMs: number): number {
  return Math.max(0, Math.floor((nextAtMs - nowMs) / 60_000))
}
