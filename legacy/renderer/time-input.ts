// Smooth time entry.
//
// WKWebView's native <input type="time"> forces the user to type the hour,
// tab/click over to the minute segment, then type the minute. That's clumsy for
// a workflow where you just want to punch in "1430". We replace the native
// control with a masked text input that accepts a continuous digit stream and
// formats it as you type (1 → 14 → 14:3 → 14:30), always producing a canonical
// "HH:MM" string in `.value` so downstream HH:MM parsing/validation is unchanged.

// Turn a raw (possibly partial) digit stream into a progressively-formatted
// "HH:MM" string. Implements the classic time-mask rules so single-digit hours
// and minutes auto-pad and advance:
//   - hour first digit 3-9  → "0X:" (e.g. 9 → "09:")
//   - hour "2" then 4-9     → "02:" (24-29 is not a valid hour) and the digit
//                             rolls into the minutes
//   - minute first digit 6-9 → "0X" (e.g. 7 → "07")
function maskTime(raw: string): string {
  const digits = raw.replace(/\D/g, '')
  let h = ''
  let m = ''
  let i = 0

  if (i < digits.length) {
    const d0 = digits[i]
    if (d0 >= '3') {
      h = '0' + d0
      i++
    } else {
      h = d0
      i++
      if (i < digits.length) {
        const d1 = digits[i]
        if (d0 === '2' && d1 >= '4') {
          h = '0' + d0 // 24-29 invalid → treat d0 as a single-digit hour
        } else {
          h += d1
          i++
        }
      }
    }
  }

  if (i < digits.length) {
    const d0 = digits[i]
    if (d0 >= '6') {
      m = '0' + d0
      i++
    } else {
      m = d0
      i++
      if (i < digits.length) {
        m += digits[i]
        i++
      }
    }
  }

  if (h === '') return ''
  if (m === '') return h.length === 2 ? `${h}:` : h
  return `${h.padStart(2, '0')}:${m}`
}

// Normalize whatever is in the field to a valid, zero-padded "HH:MM" (or empty).
function normalizeTime(raw: string): string {
  const match = raw.match(/^(\d{1,2}):?(\d{0,2})$/)
  if (!match) return ''
  const hh = Math.min(23, parseInt(match[1] || '0', 10))
  const mm = Math.min(59, parseInt(match[2] || '0', 10))
  return `${String(hh).padStart(2, '0')}:${String(mm).padStart(2, '0')}`
}

// Convert one native time input into a masked text input. Idempotent.
export function enhanceTimeInput(el: HTMLInputElement | null): void {
  if (!el || el.dataset.timeMasked === '1') return
  el.dataset.timeMasked = '1'
  el.type = 'text'
  el.inputMode = 'numeric'
  el.autocomplete = 'off'
  el.maxLength = 5
  if (!el.placeholder) el.placeholder = 'tt:mm'
  // A native value attribute like "11:00" carries over fine as text.

  el.addEventListener('input', e => {
    const isDelete = (e as InputEvent).inputType?.startsWith('delete') ?? false
    let v = maskTime(el.value)
    // Don't re-add the auto colon the user is trying to delete past.
    if (isDelete && v.endsWith(':')) v = v.slice(0, -1)
    el.value = v
  })

  el.addEventListener('blur', () => {
    const before = el.value
    el.value = normalizeTime(el.value)
    // Fire change so existing listeners (auto stop-time, duration display) run.
    if (el.value !== before) el.dispatchEvent(new Event('change', { bubbles: true }))
  })
}

// Enhance every native time input currently in the DOM under `root`.
export function enhanceTimeInputs(root: ParentNode = document): void {
  root.querySelectorAll<HTMLInputElement>('input[type="time"]').forEach(enhanceTimeInput)
}
