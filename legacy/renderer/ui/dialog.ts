/**
 * Promise-based dialogs — the replacement for window.confirm/alert/prompt.
 *
 * The native dialogs were a liability in a Tauri app: they render in the OS
 * chrome (wrong typeface, wrong colours, English-only OK/Cancel on some
 * platforms), they cannot say WHICH recording is about to be deleted in more
 * than one flat line, and on macOS they steal the window's key status in a way
 * that has already been observed to interrupt a running capture.
 *
 * One dialog root is created lazily and reused. Only one dialog is open at a
 * time — a second call queues behind the first rather than stacking, because a
 * stack of modal dialogs is never the right answer.
 *
 *   await confirmDialog({ title, message, danger: true })   -> boolean
 *   await alertDialog({ title, message })                   -> void
 *   await promptDialog({ title, defaultValue })             -> string | null
 *   await selectDialog({ title, options })                  -> string | null
 */

import { t } from '../i18n'
import {
  buildAlert,
  buildConfirm,
  buildPrompt,
  buildSelect,
  cancelButton,
  defaultButton,
  nextFocusIndex,
} from './dialog-core'
import type {
  AlertOpts,
  ConfirmOpts,
  DialogSpec,
  PromptOpts,
  SelectOpts,
} from './dialog-core'
import { prefersReducedMotion } from './motion'

export type { DialogOption } from './dialog-core'

const FOCUSABLE =
  'button:not(:disabled), [href], input:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])'

let root: HTMLElement | null = null
/** Serialises overlapping calls so a second dialog waits instead of stacking. */
let chain: Promise<unknown> = Promise.resolve()

function ensureRoot(): HTMLElement {
  if (root && root.isConnected) return root
  root = document.createElement('div')
  root.className = 'ui-dialog-root'
  root.setAttribute('aria-live', 'polite')
  document.body.appendChild(root)
  return root
}

/** Default labels, resolved at call time so a language switch is picked up. */
function labels(): { ok: string; cancel: string; confirm: string } {
  return {
    ok: t('dialog.ok', 'OK'),
    cancel: t('dialog.cancel', 'Avbryt'),
    confirm: t('dialog.confirm', 'Fortsett'),
  }
}

/**
 * Render a spec and resolve with the id of the button that closed it (or
 * `select:<optionId>` when a list row was picked).
 */
function present(spec: DialogSpec): Promise<string> {
  const run = (): Promise<string> =>
    new Promise<string>(resolve => {
      const host = ensureRoot()
      const previouslyFocused = document.activeElement as HTMLElement | null

      const backdrop = document.createElement('div')
      backdrop.className = 'ui-dialog-backdrop'

      const box = document.createElement('div')
      box.className = 'ui-dialog' + (spec.danger ? ' is-danger' : '')
      box.setAttribute('role', spec.kind === 'alert' ? 'alertdialog' : 'dialog')
      box.setAttribute('aria-modal', 'true')

      const titleEl = document.createElement('h2')
      titleEl.className = 'ui-dialog-title'
      titleEl.textContent = spec.title
      titleEl.id = 'ui-dialog-title'
      box.setAttribute('aria-labelledby', titleEl.id)
      box.appendChild(titleEl)

      if (spec.message) {
        const p = document.createElement('p')
        p.className = 'ui-dialog-message'
        // textContent, never innerHTML: these strings carry file names and
        // backend error text straight from disk.
        p.textContent = spec.message
        box.appendChild(p)
      }

      let field: HTMLInputElement | HTMLTextAreaElement | null = null
      if (spec.input) {
        field = spec.input.multiline
          ? document.createElement('textarea')
          : document.createElement('input')
        if (field instanceof HTMLInputElement) field.type = 'text'
        if (field instanceof HTMLTextAreaElement) field.rows = 3
        field.className = 'form-input ui-dialog-input'
        field.value = spec.input.value
        field.placeholder = spec.input.placeholder
        box.appendChild(field)
      }

      if (spec.options) {
        const list = document.createElement('div')
        list.className = 'ui-dialog-list'
        list.setAttribute('role', 'listbox')
        if (spec.options.length === 0) {
          const empty = document.createElement('div')
          empty.className = 'ui-dialog-list-empty'
          empty.textContent = t('dialog.noOptions', 'Ingen alternativer funnet.')
          list.appendChild(empty)
        }
        for (const opt of spec.options) {
          const row = document.createElement('button')
          row.type = 'button'
          row.className = 'ui-dialog-option'
          row.setAttribute('role', 'option')
          const label = document.createElement('span')
          label.className = 'ui-dialog-option-label'
          label.textContent = opt.label
          row.appendChild(label)
          if (opt.detail) {
            const detail = document.createElement('span')
            detail.className = 'ui-dialog-option-detail'
            detail.textContent = opt.detail
            row.appendChild(detail)
          }
          row.addEventListener('click', () => close('select:' + opt.id))
          list.appendChild(row)
        }
        box.appendChild(list)
      }

      const actions = document.createElement('div')
      actions.className = 'ui-dialog-actions'
      const buttonEls: HTMLButtonElement[] = []
      for (const b of spec.buttons) {
        const el = document.createElement('button')
        el.type = 'button'
        el.className = 'btn-' + b.variant
        el.textContent = b.label
        el.dataset.dialogButton = b.id
        el.addEventListener('click', () => close(b.id))
        actions.appendChild(el)
        buttonEls.push(el)
      }
      box.appendChild(actions)
      backdrop.appendChild(box)

      const cancelId = cancelButton(spec)?.id ?? null
      const defaultId = defaultButton(spec)?.id ?? null

      let settled = false
      const close = (id: string): void => {
        if (settled) return
        settled = true
        document.removeEventListener('keydown', onKey, true)
        backdrop.removeEventListener('mousedown', onBackdrop)
        backdrop.classList.add('is-leaving')
        const remove = (): void => {
          backdrop.remove()
          if (host.childElementCount === 0) host.classList.remove('is-open')
          // Give focus back where the user left it, so keyboard navigation
          // resumes at the control that opened the dialog.
          previouslyFocused?.focus?.()
          resolve(id)
        }
        if (prefersReducedMotion()) remove()
        else setTimeout(remove, 130)
      }

      const onBackdrop = (e: MouseEvent): void => {
        // mousedown, not click: a drag that starts inside the dialog and ends on
        // the backdrop (text selection) must not dismiss it.
        if (e.target === backdrop && cancelId) close(cancelId)
      }
      backdrop.addEventListener('mousedown', onBackdrop)

      const onKey = (e: KeyboardEvent): void => {
        if (e.key === 'Escape') {
          if (!cancelId) return
          e.preventDefault()
          e.stopPropagation()
          close(cancelId)
          return
        }
        if (e.key === 'Enter') {
          // In a multiline prompt Enter inserts a newline; Cmd/Ctrl+Enter commits.
          const inTextarea = field instanceof HTMLTextAreaElement && e.target === field
          if (inTextarea && !(e.metaKey || e.ctrlKey)) return
          if (!defaultId) return
          e.preventDefault()
          e.stopPropagation()
          close(defaultId)
          return
        }
        if (e.key === 'Tab') {
          const items = Array.from(box.querySelectorAll<HTMLElement>(FOCUSABLE))
          if (!items.length) return
          e.preventDefault()
          const current = items.indexOf(document.activeElement as HTMLElement)
          const next = nextFocusIndex(items.length, current, e.shiftKey)
          if (next >= 0) items[next].focus()
        }
      }
      // Capture phase: the app's own global Escape handler must not also fire.
      document.addEventListener('keydown', onKey, true)

      host.classList.add('is-open')
      host.appendChild(backdrop)

      // Focus the field if there is one (the user is here to type), otherwise
      // the first list row, otherwise the default button.
      requestAnimationFrame(() => {
        if (field) {
          field.focus()
          field.select?.()
          return
        }
        const firstOption = box.querySelector<HTMLElement>('.ui-dialog-option')
        if (firstOption) {
          firstOption.focus()
          return
        }
        const preferred = defaultId
          ? buttonEls.find(b => b.dataset.dialogButton === defaultId)
          : buttonEls[0]
        preferred?.focus()
      })

      // Expose the field so promptDialog can read it after the promise settles.
      pendingField = field
    })

  const next = chain.then(run, run)
  chain = next.catch(() => undefined)
  return next
}

/** Set by `present` while a prompt is on screen; read immediately after close. */
let pendingField: HTMLInputElement | HTMLTextAreaElement | null = null

/** Ask a yes/no question. Resolves true only if the confirm button was used. */
export async function confirmDialog(opts: ConfirmOpts): Promise<boolean> {
  const l = labels()
  const spec = buildConfirm({
    ...opts,
    confirmLabel: opts.confirmLabel ?? l.confirm,
    cancelLabel: opts.cancelLabel ?? l.cancel,
  })
  return (await present(spec)) === 'ok'
}

/** Tell the user something. Resolves when they acknowledge it. */
export async function alertDialog(opts: AlertOpts): Promise<void> {
  const l = labels()
  await present(buildAlert({ ...opts, okLabel: opts.okLabel ?? l.ok }))
}

/** Ask for a line of text. Resolves null when cancelled — NOT empty string,
 *  so "cleared the field" and "backed out" stay distinguishable. */
export async function promptDialog(opts: PromptOpts): Promise<string | null> {
  const l = labels()
  const spec = buildPrompt({
    ...opts,
    confirmLabel: opts.confirmLabel ?? l.ok,
    cancelLabel: opts.cancelLabel ?? l.cancel,
  })
  const result = await present(spec)
  const value = pendingField?.value ?? ''
  pendingField = null
  return result === 'ok' ? value : null
}

/** Pick one of a list. Resolves the chosen option's id, or null on cancel. */
export async function selectDialog(opts: SelectOpts): Promise<string | null> {
  const l = labels()
  const result = await present(
    buildSelect({ ...opts, cancelLabel: opts.cancelLabel ?? l.cancel }),
  )
  return result.startsWith('select:') ? result.slice('select:'.length) : null
}
