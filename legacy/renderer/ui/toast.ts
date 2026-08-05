/**
 * Toasts and banners.
 *
 * Before this, feedback was delivered by mutating the button you just pressed:
 * `flashSaved` replaced its label with "✓ Lagret" and its background with green
 * for 1.8 s. That meant the button was simultaneously the control and the
 * receipt, the layout shifted as labels changed width, and any message longer
 * than a button was impossible. flashSaved/flashMsg still exist with the same
 * signatures — they now call toast() — so every existing call site upgraded
 * without being touched.
 *
 * Two surfaces, two purposes:
 *   toast()  — transient, top-right, self-dismissing. "It worked."
 *   banner() — persistent, pinned above the page, dismissed by id when the
 *              condition clears. "Something is wrong and stays wrong."
 */

import { t } from '../i18n'

export type ToastKind = 'info' | 'success' | 'warn' | 'error'

export interface ToastAction {
  label: string
  onClick: () => void
}

export interface ToastOpts {
  /** A single inline action, e.g. "Vis mappe". */
  action?: ToastAction
  /** Override the auto-dismiss delay. Errors default to sticky. */
  durationMs?: number
}

const DEFAULT_MS: Record<ToastKind, number> = {
  info: 3200,
  success: 2600,
  warn: 5000,
  // Errors do not disappear on their own — the one message you must not miss
  // should not be the one that vanishes while you look away.
  error: 0,
}

const ICON: Record<ToastKind, string> = {
  info: 'i',
  success: '✓',
  warn: '!',
  error: '✕',
}

let stack: HTMLElement | null = null

function ensureStack(): HTMLElement {
  if (stack && stack.isConnected) return stack
  stack = document.createElement('div')
  stack.className = 'ui-toast-stack'
  stack.setAttribute('role', 'status')
  stack.setAttribute('aria-live', 'polite')
  document.body.appendChild(stack)
  return stack
}

function dismiss(el: HTMLElement): void {
  if (el.classList.contains('is-leaving')) return
  el.classList.add('is-leaving')
  setTimeout(() => el.remove(), 220)
}

/**
 * Show a transient message. Returns a dismiss function for callers that want to
 * retract it early (a progress toast replaced by its result, say).
 */
export function toast(kind: ToastKind, msg: string, opts: ToastOpts = {}): () => void {
  const host = ensureStack()

  const el = document.createElement('div')
  el.className = `ui-toast ui-toast-${kind}`

  const icon = document.createElement('span')
  icon.className = 'ui-toast-icon'
  icon.setAttribute('aria-hidden', 'true')
  icon.textContent = ICON[kind]
  el.appendChild(icon)

  const body = document.createElement('div')
  body.className = 'ui-toast-body'
  const text = document.createElement('div')
  text.className = 'ui-toast-text'
  text.textContent = msg
  body.appendChild(text)

  if (opts.action) {
    const btn = document.createElement('button')
    btn.type = 'button'
    btn.className = 'ui-toast-action'
    btn.textContent = opts.action.label
    btn.addEventListener('click', () => {
      opts.action?.onClick()
      dismiss(el)
    })
    body.appendChild(btn)
  }
  el.appendChild(body)

  const ms = opts.durationMs ?? DEFAULT_MS[kind]
  let timer: ReturnType<typeof setTimeout> | null = null
  const arm = (): void => {
    if (ms > 0) timer = setTimeout(() => dismiss(el), ms)
  }
  const disarm = (): void => {
    if (timer) clearTimeout(timer)
    timer = null
  }

  // Sticky toasts (and any with an action) get an explicit close, because
  // "click anywhere to dismiss" is undiscoverable.
  if (ms === 0 || opts.action) {
    const close = document.createElement('button')
    close.type = 'button'
    close.className = 'ui-toast-close'
    close.setAttribute('aria-label', t('toast.dismiss', 'Lukk'))
    close.textContent = '✕'
    close.addEventListener('click', () => dismiss(el))
    el.appendChild(close)
  }

  // Reading a message shouldn't race a timer.
  el.addEventListener('mouseenter', disarm)
  el.addEventListener('mouseleave', arm)

  host.appendChild(el)
  arm()

  return () => {
    disarm()
    dismiss(el)
  }
}

// ── Banners ──────────────────────────────────────────────────────────────────

export interface BannerAction {
  label: string
  onClick: () => void
}

let banners: HTMLElement | null = null

/**
 * The banner region is the FIRST child of #main, so a banner pushes the page
 * down rather than floating over it — a persistent problem should cost layout,
 * not hover over the content it is about.
 */
function ensureBanners(): HTMLElement | null {
  if (banners && banners.isConnected) return banners
  const main = document.getElementById('main')
  if (!main) return null
  const existing = document.getElementById('app-banners')
  if (existing) {
    banners = existing
    return banners
  }
  banners = document.createElement('div')
  banners.id = 'app-banners'
  banners.className = 'ui-banner-region'
  main.insertBefore(banners, main.firstChild)
  return banners
}

/**
 * Show (or update in place) a persistent banner.
 *
 * Keyed by `id` so a repeating condition — a device that keeps disconnecting —
 * refreshes one banner instead of stacking twenty.
 */
export function banner(
  id: string,
  kind: ToastKind,
  msg: string,
  actions: BannerAction[] = [],
): void {
  const host = ensureBanners()
  if (!host) return

  const existing = host.querySelector<HTMLElement>(`[data-banner-id="${CSS.escape(id)}"]`)
  const el = existing ?? document.createElement('div')
  el.dataset.bannerId = id
  el.className = `ui-banner ui-banner-${kind}`
  el.replaceChildren()

  const icon = document.createElement('span')
  icon.className = 'ui-banner-icon'
  icon.setAttribute('aria-hidden', 'true')
  icon.textContent = ICON[kind]
  el.appendChild(icon)

  const text = document.createElement('span')
  text.className = 'ui-banner-text'
  text.textContent = msg
  el.appendChild(text)

  if (actions.length) {
    const row = document.createElement('span')
    row.className = 'ui-banner-actions'
    for (const a of actions) {
      const btn = document.createElement('button')
      btn.type = 'button'
      btn.className = 'ui-banner-action'
      btn.textContent = a.label
      btn.addEventListener('click', a.onClick)
      row.appendChild(btn)
    }
    el.appendChild(row)
  }

  const close = document.createElement('button')
  close.type = 'button'
  close.className = 'ui-banner-close'
  close.setAttribute('aria-label', t('toast.dismiss', 'Lukk'))
  close.textContent = '✕'
  close.addEventListener('click', () => dismissBanner(id))
  el.appendChild(close)

  if (!existing) host.appendChild(el)
}

/** Remove a banner by id. Safe to call when none is showing. */
export function dismissBanner(id: string): void {
  const host = document.getElementById('app-banners')
  host?.querySelector(`[data-banner-id="${CSS.escape(id)}"]`)?.remove()
}
