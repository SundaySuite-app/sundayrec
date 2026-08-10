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
  // The menubar tray renders its own labels in Rust and cannot read the UI
  // language: it lives in this renderer's settings blob, which the backend's
  // curated recording settings never carried. Push it from the ONE place the
  // language actually changes, so startup and a live switch are the same path.
  // Fire-and-forget — a tray-less build answers with a harmless no-op.
  void window.api?.traySetLanguage?.(currentLang)
}

/** Raw catalogue lookup — may return a string, a plural group object, an array
 *  or undefined. `t`/`tArr`/`tn` each narrow it their own way. */
function lookup(key: string): unknown {
  return key.split('.').reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], T)
}

export function t(key: string, fallback = ''): string {
  const val = lookup(key)
  // Plural keys are OBJECTS ({one, few, many, other}) — a bare `t()` on one used
  // to stringify to "[object Object]" in the UI. Anything that is not a string
  // is not a translation, so it takes the fallback.
  return typeof val === 'string' ? val : fallback
}

/**
 * The ONE BCP-47 tag for date/number/plural formatting: bokmål for 'no' (plain
 * 'no' gives nynorsk-flavoured output in some engines), else the UI language.
 *
 * Lives here rather than in helpers.ts because `currentLang` lives here, and
 * `tn()` needs the tag: importing it back from helpers.ts would put i18n.ts in
 * an import cycle with helpers.ts (which imports `t`) and drag `ui/toast` into
 * i18n's module graph. helpers.ts re-exports it, so its ~30 call sites are
 * unchanged.
 */
export function localeTag(lang: string = currentLang): string {
  return lang === 'no' ? 'nb-NO' : lang
}

/**
 * Substitute `{name}` placeholders.
 *
 * `replaceAll`, not `replace`: the old hand-rolled `t(...).replace('{n}', …)`
 * call sites replaced only the FIRST occurrence, so any string that named the
 * same placeholder twice rendered a raw `{n}` to the operator.
 *
 * Missing-param policy: a placeholder with no matching param is LEFT VISIBLE as
 * `{n}`. The alternative (substituting '') reads as finished copy — «opptak
 * ligger i papirkurven» — and hides the bug from everyone, including the person
 * reading a screenshot. A visible `{n}` is ugly on purpose. Pinned by test.
 */
export function interpolate(template: string, params: Record<string, string | number>): string {
  let out = template
  for (const [k, v] of Object.entries(params)) out = out.replaceAll(`{${k}}`, String(v))
  return out
}

/** Cached per language — constructing Intl.PluralRules is not free and these
 *  run inside list renders. */
const pluralRules = new Map<string, Intl.PluralRules>()

export function pluralCategory(count: number, lang: string = currentLang): Intl.LDMLPluralRule {
  let rules = pluralRules.get(lang)
  if (!rules) {
    rules = new Intl.PluralRules(localeTag(lang))
    pluralRules.set(lang, rules)
  }
  return rules.select(count)
}

/**
 * Pick the right form out of a plural group.
 *
 * `node` is the raw catalogue value: a group object keyed by CLDR category
 * ({one, few, many, other} — exactly the categories that language needs), or a
 * plain string for a key that was never pluralized. Returns undefined when
 * there is nothing usable, so callers can fall back to their literal.
 *
 * Kept pure (locale data + language in, string out) so the unit gate can drive
 * every language and every boundary without a DOM.
 */
export function selectPluralForm(
  node: unknown,
  count: number,
  lang: string = currentLang,
): string | undefined {
  if (typeof node === 'string') return node
  if (!node || typeof node !== 'object' || Array.isArray(node)) return undefined
  const group = node as Record<string, unknown>
  const exact = group[pluralCategory(count, lang)]
  if (typeof exact === 'string') return exact
  // `other` is the universal fallback: French 'many' (≥1e6) and Polish 'other'
  // (fractions) are categories real counts in this app never reach, so a
  // catalogue may legitimately omit them.
  return typeof group['other'] === 'string' ? group['other'] : undefined
}

/** Interpolating `t`. Replaces the hand-rolled `t(k, f).replace('{n}', …)` chain. */
export function tf(
  key: string,
  params: Record<string, string | number>,
  fallback = '',
): string {
  return interpolate(t(key, fallback), params)
}

/**
 * Count-aware `t`. Looks up `key.<CLDR category>` for `count` in the active
 * language, falling back to `key.other`, then to a flat `key`, then to
 * `fallback`. `{n}` is pre-bound to `count`; `params` can add more (and may
 * override `n`).
 *
 * This is what makes Polish correct: «2 nagrania» (few) vs «5 nagrań» (many)
 * was one string before, so 2–4 and 22–24 rendered the wrong noun form.
 */
export function tn(
  key: string,
  count: number,
  params: Record<string, string | number> = {},
  fallback = '',
): string {
  const form = selectPluralForm(lookup(key), count, currentLang) ?? fallback
  return interpolate(form, { n: count, ...params })
}

export function tArr(key: string, fallback: string[]): string[] {
  const val = key.split('.').reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], T)
  return Array.isArray(val) ? val as string[] : fallback
}

let _applyTranslations = (): void => {}

export function setApplyHook(fn: () => void): void {
  _applyTranslations = fn
}

/**
 * Live-painted surfaces vs. the data-i18n pass.
 *
 * `applyTranslations()` rewrites every `[data-i18n]` node from the locale
 * table — which is exactly wrong for the ~20 nodes whose text is painted from
 * STATE (the sidebar status, the wake card, the stop button mid-finalize…):
 * a language switch used to reset them to their markup defaults, erasing live
 * truth. The house fix (index.html's update-channel comment) is "repaint via
 * hook": modules register a cheap, synchronous repaint-from-cached-state here,
 * and it runs AFTER the data-i18n pass — so the cold-render default still
 * translates, and live state always wins the same synchronous frame.
 *
 * Callbacks must be idempotent, synchronous and safe to run at any time
 * (including before the page they paint has been visited).
 */
const localeRepaints = new Set<() => void>()

export function onLocaleApplied(fn: () => void): void {
  localeRepaints.add(fn)
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
  // After the attribute pass, so live state repaints over the defaults.
  for (const fn of localeRepaints) fn()
}
