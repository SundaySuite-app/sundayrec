/**
 * One save model for every settings control.
 *
 * `bindSetting(el, opts)` wires a control to the settings store: it commits on
 * change (immediately for a toggle/select/radio, on release for a slider,
 * debounced for free text), persists through the existing debounced save in
 * `state.ts`, and then shows a small inline «Lagret ✓» chip next to the control
 * — the same receipt the channel grid has used since v0.7.0, now everywhere.
 *
 * What it replaces:
 *   • audio/video: silent auto-save. The write happened; nothing said so.
 *   • Filer/Publisering/System/Varsler: a dirty-footer with Lagre + Avbryt,
 *     where several controls auto-saved anyway and «Avbryt» could not revert
 *     what had already been written.
 *   • Sunday-suite: three inline «Lagre» buttons with three different labels.
 *
 * The exceptions that KEEP an explicit Lagre/Avbryt are the ones where a
 * half-typed value is actively harmful: the schedule slot editor, the e-mail
 * server fields and the integration URL/key pairs. Those are bordered cards,
 * not auto-apply controls, and they do not use this module.
 *
 * The decisions (when to commit, how to coerce, when to ask first) live in
 * `bind-setting-core.ts` and are unit-tested; this file is the DOM around them.
 */

import { t, tf, tn } from '../i18n'
import { saveSettingsDebounced } from '../state'
import { getNextRecordingState } from '../status/next-recording'
import { WAKE_LEAD_MINUTES } from '../status/next-recording-core'
import { confirmDialog } from './dialog'
import { setFieldError } from './field-error'
import { toast } from './toast'
import {
  coerceValue,
  controlKindOf,
  guardReasonFor,
  isRealChange,
  minutesUntil,
  planCommit,
  SAVE_COALESCE_MS,
  SAVED_CHIP_MS,
  type ControlKind,
  type SettingValue,
} from './bind-setting-core'

export type { SettingValue } from './bind-setting-core'

/** What a guard wants to ask before the change is applied. */
export interface GuardDescriptor {
  title: string
  message?: string
  confirmLabel?: string
  cancelLabel?: string
}

export interface BindContext {
  el: HTMLElement
  /** The last committed value, before this change. */
  previous: SettingValue
}

export interface BindSettingOpts<T = SettingValue> {
  /** Settings key this control owns. Used for the chip's aria-label + logging. */
  key?: string
  /** Narrow the raw control value before validation/apply. */
  coerce?: (raw: SettingValue, el: HTMLElement) => T
  /** Return a message to reject the value (shown inline under the field). */
  validate?: (value: T, el: HTMLElement) => string | null
  /** Write the value into the settings object. Called only for a real change. */
  apply: (value: T, el: HTMLElement) => void
  /**
   * Persist. Defaults to the shared debounced `saveSettings` in state.ts —
   * override for a surface with its own store (the telemetry consent row).
   */
  persist?: () => Promise<boolean>
  /** Runs after a successful save (refresh Home, re-derive the schedule …). */
  after?: (value: T) => void
  /** Ask before applying. Return null to apply without a question. */
  confirmIf?: (value: T, ctx: BindContext) => GuardDescriptor | null
  /** Where the «Lagret ✓» chip goes. Default: the control's own label/row. */
  chipHost?: HTMLElement | null | (() => HTMLElement | null)
  /** Commit on the event itself even for a text field. */
  immediate?: boolean
  /** Explicit commit debounce, overriding the control kind's default. */
  debounceMs?: number
  /** Suppress the chip (the surface shows its own receipt). */
  silent?: boolean
  /** Put the control back after a declined guard / rejected value. */
  revert?: (previous: SettingValue, el: HTMLElement) => void
}

/**
 * Every live binding, so `resyncBoundSettings()` can refresh their baselines
 * after an `applyXSettingsToUI()` pass has rewritten the DOM from settings.
 * Without that, a control the user flips back to the value it held when the
 * page was first wired would compare equal and never be written.
 */
const bindings = new Set<{ resync: () => void }>()

/** Re-read every bound control's current value as its new baseline. */
export function resyncBoundSettings(): void {
  for (const b of bindings) b.resync()
}

/** Resolve an element from an id or a node. */
function resolveEl(target: HTMLElement | string | null): HTMLElement | null {
  if (!target) return null
  return typeof target === 'string' ? document.getElementById(target) : target
}

function rawOf(el: HTMLElement): { value: string; checked?: boolean } {
  const input = el as HTMLInputElement
  return { value: input.value ?? '', checked: input.checked }
}

// ── The «Lagret ✓» chip ──────────────────────────────────────────────────────

/**
 * Pick where the receipt goes. In order of preference: the toggle's own title
 * line (there is slack there and it reads as part of the setting), the label
 * that precedes the field, then the field's container. Anything is better than
 * a toast for a change this small — a toast for every toggle would be a
 * notification storm.
 */
function defaultChipHost(el: HTMLElement): HTMLElement | null {
  const toggleRow = el.closest<HTMLElement>('.toggle-row')
  const title = toggleRow?.querySelector<HTMLElement>('.toggle-title')
  if (title) return title
  // A `.form-label` immediately before the field (or before its wrapper row).
  const wrapper = el.closest<HTMLElement>('.folder-row, .inline-field-row, .pass-row') ?? el
  const prev = wrapper.previousElementSibling as HTMLElement | null
  if (prev?.classList.contains('form-label')) return prev
  const optionCards = el.closest<HTMLElement>('.option-cards, .quality-scale')
  if (optionCards) {
    const label = optionCards.previousElementSibling as HTMLElement | null
    if (label?.classList.contains('form-label') || label?.classList.contains('card-title')) {
      return label
    }
  }
  return el.parentElement
}

/** Flash «Lagret ✓» in `host`, replacing any chip already showing there. */
export function showSavedChip(host: HTMLElement | null, label?: string): void {
  if (!host) return
  host.querySelector('.setting-saved-chip')?.remove()
  const chip = document.createElement('span')
  chip.className = 'setting-saved-chip'
  chip.setAttribute('role', 'status')
  chip.textContent = label ?? t('general.savedChip', 'Lagret ✓')
  host.appendChild(chip)
  requestAnimationFrame(() => chip.classList.add('is-visible'))
  window.setTimeout(() => {
    chip.classList.remove('is-visible')
    window.setTimeout(() => chip.remove(), 240)
  }, SAVED_CHIP_MS)
}

// ── Guards ───────────────────────────────────────────────────────────────────

/**
 * The guard the mission calls for: a change that can cost you the recording
 * about to start asks first. Uses the shared next-recording store, so it knows
 * about one-off specials too, not just weekly slots.
 *
 * `what` names the change in the question ("Bytte lydenhet", "Slå av …") so the
 * dialog is about the thing the user just clicked, not a generic warning.
 */
export function recordingImminentGuard(
  what: string,
): (value: SettingValue, ctx: BindContext) => GuardDescriptor | null {
  return () => {
    const state = getNextRecordingState()
    const reason = guardReasonFor(
      { isRecording: state.isRecording, nextAtMs: state.next?.atMs ?? null },
      Date.now(),
      WAKE_LEAD_MINUTES,
    )
    if (!reason) return null
    const message =
      reason === 'recording'
        ? t(
            'guard.duringRecording',
            'Et opptak pågår akkurat nå. Endringen kan avbryte eller ødelegge det som tas opp.',
          )
        : tn(
            'guard.beforeRecording',
            minutesUntil(state.next?.atMs ?? Date.now(), Date.now()),
            {},
            'Neste planlagte opptak starter om {n} minutter. Endringen rekker å slå ut på det opptaket.',
          )
    return {
      title: tf('guard.title', { what }, '{what} nå?'),
      message,
      confirmLabel: t('guard.confirm', 'Ja, endre likevel'),
      cancelLabel: t('guard.cancel', 'Ikke nå'),
    }
  }
}

/**
 * The same guard, imperative — for surfaces that are not form controls. The
 * audio device picker is a list of clickable cards, not a `<select>`, so it
 * cannot go through `bindSetting`; it asks here instead. Resolves to whether
 * the change should proceed.
 */
export async function confirmIfRecordingImminent(what: string): Promise<boolean> {
  const descriptor = recordingImminentGuard(what)(null, {
    el: document.body,
    previous: null,
  })
  if (!descriptor) return true
  return confirmDialog({
    title: descriptor.title,
    message: descriptor.message,
    confirmLabel: descriptor.confirmLabel,
    cancelLabel: descriptor.cancelLabel,
    danger: true,
  })
}

// ── The binder ───────────────────────────────────────────────────────────────

/**
 * Bind one control. Returns an unbind function; call it if the control is ever
 * re-rendered, so listeners never stack.
 */
export function bindSetting<T = SettingValue>(
  target: HTMLElement | string | null,
  opts: BindSettingOpts<T>,
): () => void {
  const el = resolveEl(target)
  if (!el) return () => {}

  const kind = controlKindOf({ tag: el.tagName, type: (el as HTMLInputElement).type })
  return bindControls([el], el, kind, opts)
}

/**
 * Bind a radio group by `name`. The group behaves as one setting: the value is
 * whichever option is checked, and the receipt appears once, next to the group.
 */
export function bindRadioGroup<T = SettingValue>(
  name: string,
  opts: BindSettingOpts<T>,
): () => void {
  const inputs = Array.from(
    document.querySelectorAll<HTMLInputElement>(`input[name="${name}"]`),
  )
  if (!inputs.length) return () => {}
  const read = (): HTMLElement =>
    (document.querySelector<HTMLInputElement>(`input[name="${name}"]:checked`) ?? inputs[0]) as HTMLElement
  return bindControls(inputs, read(), 'radio', {
    ...opts,
    // The chip belongs to the group, not to whichever radio happened to win.
    chipHost: opts.chipHost ?? (() => defaultChipHost(inputs[0])),
    revert:
      opts.revert ??
      ((previous) => {
        const match = inputs.find(i => i.value === String(previous))
        if (match) match.checked = true
      }),
  })
}

function bindControls<T>(
  listenOn: HTMLElement[],
  valueEl: HTMLElement,
  kind: ControlKind,
  opts: BindSettingOpts<T>,
): () => void {
  const plan = planCommit(kind, { immediate: opts.immediate, debounceMs: opts.debounceMs })

  const readValue = (): SettingValue => {
    if (kind === 'radio') {
      const name = (listenOn[0] as HTMLInputElement).name
      const checked = document.querySelector<HTMLInputElement>(`input[name="${name}"]:checked`)
      return checked?.value ?? null
    }
    return coerceValue(kind, rawOf(valueEl))
  }

  let previous = readValue()
  let timer: ReturnType<typeof setTimeout> | null = null
  let busy = false

  const revert = (): void => {
    if (opts.revert) {
      opts.revert(previous, valueEl)
      return
    }
    const input = valueEl as HTMLInputElement
    if (kind === 'toggle') input.checked = previous === true
    else input.value = previous === null ? '' : String(previous)
  }

  const commit = async (): Promise<void> => {
    if (busy) return
    const raw = readValue()
    if (!isRealChange(previous, raw)) return

    const value = (opts.coerce ? opts.coerce(raw, valueEl) : raw) as T

    const message = opts.validate?.(value, valueEl) ?? null
    if (message) {
      setFieldError(valueEl, message)
      return
    }
    setFieldError(valueEl, null)

    const guard = opts.confirmIf?.(value, { el: valueEl, previous }) ?? null
    if (guard) {
      busy = true
      const ok = await confirmDialog({
        title: guard.title,
        message: guard.message,
        confirmLabel: guard.confirmLabel,
        cancelLabel: guard.cancelLabel,
        danger: true,
      })
      busy = false
      if (!ok) {
        revert()
        return
      }
    }

    opts.apply(value, valueEl)
    previous = raw

    const persisted = await (opts.persist
      ? opts.persist()
      : saveSettingsDebounced(SAVE_COALESCE_MS))
    if (persisted === false) {
      toast('error', t('general.saveFailed', 'Kunne ikke lagre innstillingen'))
      return
    }
    if (!opts.silent) {
      const host =
        typeof opts.chipHost === 'function'
          ? opts.chipHost()
          : (opts.chipHost ?? defaultChipHost(valueEl))
      showSavedChip(host)
    }
    opts.after?.(value)
  }

  const onEvent = (): void => {
    if (plan.debounceMs === 0) {
      void commit()
      return
    }
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => {
      timer = null
      void commit()
    }, plan.debounceMs)
  }

  for (const node of listenOn) {
    for (const ev of plan.events) node.addEventListener(ev, onEvent)
  }

  const entry = { resync: () => { previous = readValue() } }
  bindings.add(entry)

  return () => {
    if (timer) clearTimeout(timer)
    bindings.delete(entry)
    for (const node of listenOn) {
      for (const ev of plan.events) node.removeEventListener(ev, onEvent)
    }
  }
}
