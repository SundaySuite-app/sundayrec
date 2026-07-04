// Only the default/fallback locale is bundled eagerly. The other six are
// dynamic-imported on first use (see LAZY_LOADERS) — that keeps ~280 KB of
// unused locale JSON OUT of the initial bundle, the single biggest startup win.
import noLocale from '../locales/no.json'

type LocaleData = Record<string, unknown>

const LOCALE_MAP: Record<string, LocaleData> = {
  no: noLocale as LocaleData,
}

/** Dynamic-import loaders for the non-default locales. Vite emits each as its
 *  own chunk, fetched only when that language is selected. */
const LAZY_LOADERS: Record<string, () => Promise<{ default: unknown }>> = {
  en: () => import('../locales/en.json'),
  fr: () => import('../locales/fr.json'),
  de: () => import('../locales/de.json'),
  sv: () => import('../locales/sv.json'),
  da: () => import('../locales/da.json'),
  pl: () => import('../locales/pl.json'),
}

export let T: LocaleData = LOCALE_MAP['no']
export let currentLang = 'no'

/**
 * Activate a locale, lazy-loading it on first use. Async now (was sync) because
 * the non-default locales are fetched on demand. Always resolves — an unknown
 * language or a failed import falls back to the eagerly-bundled `no`. Callers at
 * startup should await this before building localized UI; the language-switch
 * caller can fire-and-forget (applyTranslations re-applies when it resolves).
 */
export async function loadLocale(lang: string): Promise<void> {
  if (!LOCALE_MAP[lang]) {
    const loader = LAZY_LOADERS[lang]
    if (loader) {
      try {
        LOCALE_MAP[lang] = (await loader()).default as LocaleData
      } catch {
        // fall through to the 'no' fallback below
      }
    }
  }
  T = LOCALE_MAP[lang] ?? LOCALE_MAP['no']
  currentLang = LOCALE_MAP[lang] ? lang : 'no'
  applyTranslations()
}

export function t(key: string, fallback = ''): string {
  const val = key.split('.').reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], T)
  return (val as string) ?? fallback
}

export function tArr(key: string, fallback: string[]): string[] {
  const val = key.split('.').reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], T)
  return Array.isArray(val) ? val as string[] : fallback
}

let _applyTranslations = (): void => {}

export function setApplyHook(fn: () => void): void {
  _applyTranslations = fn
}

function applyTranslations(): void {
  document.querySelectorAll('[data-i18n]').forEach(el => {
    const key = (el as HTMLElement).dataset.i18n!
    const v = t(key); if (v) el.textContent = v
  })
  document.querySelectorAll('[data-i18n-placeholder]').forEach(el => {
    const key = (el as HTMLInputElement).dataset.i18nPlaceholder!
    const v = t(key); if (v) (el as HTMLInputElement).placeholder = v
  })
  document.querySelectorAll('[data-i18n-title]').forEach(el => {
    const key = (el as HTMLElement).dataset.i18nTitle!
    const v = t(key); if (v) el.setAttribute('title', v)
  })
  document.querySelectorAll('[data-i18n-aria-label]').forEach(el => {
    const key = (el as HTMLElement).dataset.i18nAriaLabel!
    const v = t(key); if (v) el.setAttribute('aria-label', v)
  })
  _applyTranslations()
}
