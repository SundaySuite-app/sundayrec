import { t, currentLang } from './i18n'
import { toast } from './ui/toast'

export function escHtml(str: unknown): string {
  return String(str ?? '').replace(/[&<>"']/g, m =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[m] ?? m)
  )
}

export function setVal(id: string, val: unknown): void {
  const el = document.getElementById(id) as HTMLInputElement | null
  if (el && val !== undefined && val !== null) el.value = String(val)
}

export function setRadio(name: string, value: string): void {
  const r = document.querySelector<HTMLInputElement>(`input[name="${name}"][value="${value}"]`)
  if (r) r.checked = true
}

export function updateSliderLabel(sliderId: string, labelId: string, suffix = ''): void {
  const el  = document.getElementById(sliderId) as HTMLInputElement | null
  const lbl = document.getElementById(labelId)
  if (el && lbl) lbl.textContent = el.value + suffix
}

/** Strip a leading ✓ / ✕ / ⚠ from legacy messages — the toast draws its own
 *  status icon, and two in a row reads as a typo. */
function stripStatusGlyph(msg: string): string {
  return msg.replace(/^[✓✔✕✖×⚠!]\s*/u, '')
}

/**
 * "Saved" feedback.
 *
 * The button argument is kept so the ~5 call sites need no edit, but it is no
 * longer used: feedback now goes to a toast instead of overwriting the label of
 * the button you just pressed. That button was both the control and the
 * receipt — its width changed as the label swapped, and nothing longer than a
 * button caption could ever be said.
 */
export function flashSaved(_btn?: HTMLElement | null): void {
  toast('success', t('general.saved', 'Lagret'))
}

/** As flashSaved, but with a caller-supplied message. `ok === false` raises an
 *  error toast, which is sticky — a failure you can actually finish reading. */
export function flashMsg(_btn: HTMLElement | null, msg: string, ok = true): void {
  toast(ok ? 'success' : 'error', stripStatusGlyph(msg))
}

export function fmtDate(iso: string): string {
  if (!iso) return '—'
  return new Date(iso).toLocaleDateString(currentLang === 'no' ? 'nb-NO' : currentLang, {
    weekday: 'short', year: 'numeric', month: 'short', day: 'numeric'
  })
}

export function fmtCountdown(ms: number): string {
  if (ms <= 0) return ''
  const totalSec = Math.floor(ms / 1000)
  const d  = Math.floor(totalSec / 86400)
  const h  = Math.floor((totalSec % 86400) / 3600)
  const m  = Math.floor((totalSec % 3600) / 60)
  const s  = totalSec % 60
  const ss = String(s).padStart(2, '0')
  const mm = String(m).padStart(2, '0')

  const uYr = t('time.yr', 'år')
  const uMo = t('time.mo', 'mnd.')
  const uWk = t('time.wk', 'u')
  const uD  = t('time.d',  'd')
  const uH  = t('time.h',  't')
  const uM  = t('time.m',  'm')
  const uS  = t('time.s',  's')

  if (d >= 365) {
    const yr  = Math.floor(d / 365)
    const mth = Math.round((d % 365) / 30)
    return mth > 0 ? `${yr} ${uYr} ${mth} ${uMo}` : `${yr} ${uYr}`
  }
  if (d >= 30) {
    const mth = Math.floor(d / 30); const rem = d % 30
    return rem > 0 ? `${mth} ${uMo} ${rem} ${uD}` : `${mth} ${uMo}`
  }
  if (d >= 7)  { const wk = Math.floor(d / 7); const rem = d % 7; return rem > 0 ? `${wk} ${uWk} ${rem} ${uD}` : `${wk} ${uWk}` }
  if (d >= 1)  { return h > 0 ? `${d} ${uD} ${h}${uH}` : `${d} ${uD}` }
  if (h > 0)   return `${h}${uH} ${mm}${uM} ${ss}${uS}`
  if (m > 0)   return `${m}${uM} ${ss}${uS}`
  return `${ss}${uS}`
}

export function fmtStorageHours(hours: number): string {
  const uH = t('time.h', 't')
  if (hours >= 8760) {
    const yr = hours / 8760
    return yr >= 10
      ? `${Math.round(yr)} ${t('time.years', 'år')}`
      : `${yr.toFixed(1)} ${t('time.years', 'år')}`
  }
  if (hours >= 720) return `${Math.round(hours / 720)} ${t('time.months', 'måneder')}`
  if (hours >= 168) return `${Math.round(hours / 168)} ${t('time.weeks', 'uker')}`
  if (hours >= 24)  return `${Math.round(hours / 24)} ${t('time.days', 'dager')}`
  return `${hours}${uH}`
}

export function isoDate(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

// setupDirtyBar was removed with the dirty-footer save model (Fase 3, 2026-08).
// It marked a `.page-footer` dirty on any input inside a settings tab — but half
// those controls had already auto-saved, so the footer claimed unsaved work that
// did not exist, and its «Avbryt» could not undo the write that had. Settings now
// auto-apply through `ui/bind-setting.ts` with an inline «Lagret ✓», and the three
// places that genuinely need staging (schedule slot editor, SMTP server fields,
// integration URL/key pairs) carry their own explicit Lagre/Avbryt card.
