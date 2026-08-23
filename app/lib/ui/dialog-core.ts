/**
 * Dialog model — PURE logic, no DOM.
 *
 * Everything about *what* a dialog is (title, message, which buttons, which one
 * is the default, which one Escape maps to) is decided here so it can be unit
 * tested in the node-environment vitest gate. `dialog.ts` owns the rendering and
 * nothing else.
 *
 * The button order below is deliberate and NOT locale-dependent: cancel first,
 * confirm last, so the affirmative action always sits where the eye ends up on
 * a left-to-right read and destructive confirms never land under the cursor's
 * resting position.
 */

export type DialogKind = 'confirm' | 'alert' | 'prompt' | 'select'
export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger'

export interface DialogButton {
  /** Stable id resolved back to the caller. 'ok' | 'cancel' for the built-ins. */
  id: string
  label: string
  variant: ButtonVariant
  /** Enter activates this button. Exactly one per dialog. */
  isDefault: boolean
  /** Escape (and backdrop click) activate this button. At most one per dialog. */
  isCancel: boolean
}

/** One row in a `selectDialog` list. */
export interface DialogOption {
  id: string
  label: string
  /** Second line under the label — a path, a resolution, an NDI address. */
  detail?: string
}

export interface DialogSpec {
  kind: DialogKind
  title: string
  /** Optional body copy. Rendered as plain text, never as HTML. */
  message?: string
  buttons: DialogButton[]
  /** prompt only. */
  input?: { value: string; placeholder: string; multiline: boolean }
  /** select only. */
  options?: DialogOption[]
  /** Destructive framing: red confirm button, red title accent. */
  danger: boolean
}

interface CommonOpts {
  title: string
  message?: string
}

export interface ConfirmOpts extends CommonOpts {
  confirmLabel?: string
  cancelLabel?: string
  /** Red confirm button. Also makes CANCEL the Enter default — a destructive
   *  action must never be one stray keypress away. */
  danger?: boolean
}

export interface AlertOpts extends CommonOpts {
  okLabel?: string
  /** Alerts that report a failure get the red accent but no danger button. */
  tone?: 'info' | 'error'
}

export interface PromptOpts extends CommonOpts {
  defaultValue?: string
  placeholder?: string
  multiline?: boolean
  confirmLabel?: string
  cancelLabel?: string
}

export interface SelectOpts extends CommonOpts {
  options: DialogOption[]
  cancelLabel?: string
}

/** Fallback labels. Callers pass translated strings; these keep the builders
 *  usable (and testable) without an i18n bundle loaded. */
const FALLBACK = {
  ok: 'OK',
  cancel: 'Avbryt',
  confirm: 'Fortsett',
} as const

export function buildConfirm(o: ConfirmOpts): DialogSpec {
  const danger = o.danger === true
  return {
    kind: 'confirm',
    title: o.title,
    message: o.message,
    danger,
    buttons: [
      {
        id: 'cancel',
        label: o.cancelLabel ?? FALLBACK.cancel,
        variant: danger ? 'secondary' : 'ghost',
        // On a destructive confirm the SAFE button is what Enter hits.
        isDefault: danger,
        isCancel: true,
      },
      {
        id: 'ok',
        label: o.confirmLabel ?? FALLBACK.confirm,
        variant: danger ? 'danger' : 'primary',
        isDefault: !danger,
        isCancel: false,
      },
    ],
  }
}

export function buildAlert(o: AlertOpts): DialogSpec {
  return {
    kind: 'alert',
    title: o.title,
    message: o.message,
    danger: o.tone === 'error',
    buttons: [
      {
        id: 'ok',
        label: o.okLabel ?? FALLBACK.ok,
        variant: 'primary',
        isDefault: true,
        // An alert has one button, so Escape must resolve through it.
        isCancel: true,
      },
    ],
  }
}

export function buildPrompt(o: PromptOpts): DialogSpec {
  return {
    kind: 'prompt',
    title: o.title,
    message: o.message,
    danger: false,
    input: {
      value: o.defaultValue ?? '',
      placeholder: o.placeholder ?? '',
      multiline: o.multiline === true,
    },
    buttons: [
      {
        id: 'cancel',
        label: o.cancelLabel ?? FALLBACK.cancel,
        variant: 'ghost',
        isDefault: false,
        isCancel: true,
      },
      {
        id: 'ok',
        label: o.confirmLabel ?? FALLBACK.ok,
        variant: 'primary',
        isDefault: true,
        isCancel: false,
      },
    ],
  }
}

export function buildSelect(o: SelectOpts): DialogSpec {
  return {
    kind: 'select',
    title: o.title,
    message: o.message,
    danger: false,
    options: o.options,
    // No confirm button: picking a row IS the confirmation. One click instead of
    // the old prompt()'s "read the list, type the number, press OK".
    buttons: [
      {
        id: 'cancel',
        label: o.cancelLabel ?? FALLBACK.cancel,
        variant: 'ghost',
        isDefault: false,
        isCancel: true,
      },
    ],
  }
}

/** The button Enter activates, or null when the spec has none. */
export function defaultButton(spec: DialogSpec): DialogButton | null {
  return spec.buttons.find(b => b.isDefault) ?? null
}

/** The button Escape and backdrop-click activate, or null. */
export function cancelButton(spec: DialogSpec): DialogButton | null {
  return spec.buttons.find(b => b.isCancel) ?? null
}

/**
 * Wrap focus inside a list of focusable elements.
 *
 * Extracted from the DOM layer so the trap's arithmetic — the part that
 * actually breaks — is testable: given N items, the current index and a
 * direction, which index gets focus next.
 */
export function nextFocusIndex(count: number, current: number, backwards: boolean): number {
  if (count <= 0) return -1
  // A focus that has escaped the dialog (current === -1) re-enters at the edge
  // the user is travelling towards.
  if (current < 0 || current >= count) return backwards ? count - 1 : 0
  return backwards ? (current - 1 + count) % count : (current + 1) % count
}
