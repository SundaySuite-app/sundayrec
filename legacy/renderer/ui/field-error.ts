/**
 * Field-level validation messages.
 *
 * A modal that says «Ugyldig tidspunkt» tells you that something on a form of
 * six fields is wrong, then takes the form away while you read it. The message
 * belongs under the field it is about, in red, tied to the input with
 * `aria-describedby` so a screen reader reads it as part of that field.
 *
 * Dialogs are for decisions ("delete this?"), not for corrections.
 */

const CLASS = 'field-error'

function errorIdFor(el: HTMLElement): string {
  if (el.id) return `${el.id}-error`
  const generated = `field-${Math.random().toString(36).slice(2, 9)}`
  el.id = generated
  return `${generated}-error`
}

/** The row the message should sit under — the wrapper when the field is in one. */
function hostFor(el: HTMLElement): HTMLElement {
  return (
    el.closest<HTMLElement>('.folder-row, .inline-field-row, .pass-row, .prep-jingle-row') ?? el
  )
}

/**
 * Show `message` under `el`, or clear it with `null`. Idempotent: calling it
 * twice with the same message leaves one node behind, not two.
 */
export function setFieldError(el: HTMLElement | string | null, message: string | null): void {
  const field = typeof el === 'string' ? document.getElementById(el) : el
  if (!field) return
  const id = errorIdFor(field)
  let node = document.getElementById(id)

  if (!message) {
    node?.remove()
    field.classList.remove('has-field-error')
    field.removeAttribute('aria-invalid')
    const described = (field.getAttribute('aria-describedby') ?? '')
      .split(/\s+/)
      .filter(x => x && x !== id)
      .join(' ')
    if (described) field.setAttribute('aria-describedby', described)
    else field.removeAttribute('aria-describedby')
    return
  }

  if (!node) {
    node = document.createElement('div')
    node.id = id
    node.className = CLASS
    node.setAttribute('role', 'alert')
    const host = hostFor(field)
    host.after(node)
  }
  node.textContent = message
  field.classList.add('has-field-error')
  field.setAttribute('aria-invalid', 'true')
  const described = (field.getAttribute('aria-describedby') ?? '').split(/\s+/).filter(Boolean)
  if (!described.includes(id)) {
    field.setAttribute('aria-describedby', [...described, id].join(' '))
  }
}

/** Clear every field error inside `scope` — used when a form panel reopens. */
export function clearFieldErrors(scope: HTMLElement | string | null): void {
  const root = typeof scope === 'string' ? document.getElementById(scope) : scope
  if (!root) return
  root.querySelectorAll<HTMLElement>('.' + CLASS).forEach(n => n.remove())
  root.querySelectorAll<HTMLElement>('.has-field-error').forEach(f => {
    f.classList.remove('has-field-error')
    f.removeAttribute('aria-invalid')
    f.removeAttribute('aria-describedby')
  })
}
