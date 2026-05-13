type LocaleData = Record<string, unknown>

const LOCALES: Record<string, LocaleData> = {}
export let T: LocaleData = {}
export let currentLang = 'no'

export async function loadLocale(lang: string, isFallback = false): Promise<void> {
  if (!LOCALES[lang]) {
    try {
      const r = await fetch(`../locales/${lang}.json`)
      LOCALES[lang] = await r.json() as LocaleData
    } catch {
      if (!isFallback && lang !== 'no') return loadLocale('no', true)
      return
    }
  }
  T = LOCALES[lang]
  currentLang = lang
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
  _applyTranslations()
}
