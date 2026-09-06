// Only the default/fallback locale is bundled eagerly. The other six are
// dynamic-imported on first use (see LAZY_LOADERS) — that keeps ~280 KB of
// unused locale JSON OUT of the initial bundle, the single biggest startup win.
import noLocale from "../../legacy/locales/no.json";

type LocaleData = Record<string, unknown>;

const LOCALE_MAP: Record<string, LocaleData> = {
  no: noLocale as LocaleData,
};

/** Dynamic-import loaders for the non-default locales. Vite emits each as its
 *  own chunk, fetched only when that language is selected. */
const LAZY_LOADERS: Record<string, () => Promise<{ default: unknown }>> = {
  en: () => import("../../legacy/locales/en.json"),
  fr: () => import("../../legacy/locales/fr.json"),
  de: () => import("../../legacy/locales/de.json"),
  sv: () => import("../../legacy/locales/sv.json"),
  da: () => import("../../legacy/locales/da.json"),
  pl: () => import("../../legacy/locales/pl.json"),
};

export let T: LocaleData = LOCALE_MAP["no"];
export let currentLang = "no";

/**
 * Load a locale's CATALOGUE and make it active.
 *
 * This used to be the DATA half of `loadLocale`, split out in S1a because the
 * other half — `applyTranslations()`, which walked the document rewriting every
 * `[data-i18n]` node — is meaningless in a Preact tree and would have dragged
 * `document` into the node-env unit gate for nothing. Fase B deleted the DOM
 * half along with the shell that had the attributes, so this is the whole
 * function now.
 *
 * `app/i18n/index.ts` awaits this and only THEN flips its `locale` signal, so a
 * render can never happen with the new language and the old catalogue. It also
 * pushes the language to the tray afterwards — the one thing `loadLocale` did
 * that was not about the DOM.
 */
export async function loadLocaleCatalogue(lang: string): Promise<void> {
  if (!LOCALE_MAP[lang]) {
    const loader = LAZY_LOADERS[lang];
    if (loader) {
      try {
        LOCALE_MAP[lang] = (await loader()).default as LocaleData;
      } catch {
        // fall through to the 'no' fallback below
      }
    }
  }
  T = LOCALE_MAP[lang] ?? LOCALE_MAP["no"];
  currentLang = LOCALE_MAP[lang] ? lang : "no";
}

/** Raw catalogue lookup — may return a string, a plural group object, an array
 *  or undefined. `t`/`tArr`/`tn` each narrow it their own way. */
function lookup(key: string): unknown {
  return key
    .split(".")
    .reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], T);
}

export function t(key: string, fallback = ""): string {
  const val = lookup(key);
  // Plural keys are OBJECTS ({one, few, many, other}) — a bare `t()` on one used
  // to stringify to "[object Object]" in the UI. Anything that is not a string
  // is not a translation, so it takes the fallback.
  return typeof val === "string" ? val : fallback;
}

/**
 * The ONE BCP-47 tag for date/number/plural formatting: bokmål for 'no' (plain
 * 'no' gives nynorsk-flavoured output in some engines), else the UI language.
 *
 * Lives here because `currentLang` lives here and `tn()` needs the tag.
 */
export function localeTag(lang: string = currentLang): string {
  return lang === "no" ? "nb-NO" : lang;
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
export function interpolate(
  template: string,
  params: Record<string, string | number>,
): string {
  let out = template;
  for (const [k, v] of Object.entries(params))
    out = out.replaceAll(`{${k}}`, String(v));
  return out;
}

/** Cached per language — constructing Intl.PluralRules is not free and these
 *  run inside list renders. */
const pluralRules = new Map<string, Intl.PluralRules>();

export function pluralCategory(
  count: number,
  lang: string = currentLang,
): Intl.LDMLPluralRule {
  let rules = pluralRules.get(lang);
  if (!rules) {
    rules = new Intl.PluralRules(localeTag(lang));
    pluralRules.set(lang, rules);
  }
  return rules.select(count);
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
  if (typeof node === "string") return node;
  if (!node || typeof node !== "object" || Array.isArray(node))
    return undefined;
  const group = node as Record<string, unknown>;
  const exact = group[pluralCategory(count, lang)];
  if (typeof exact === "string") return exact;
  // `other` is the universal fallback: French 'many' (≥1e6) and Polish 'other'
  // (fractions) are categories real counts in this app never reach, so a
  // catalogue may legitimately omit them.
  return typeof group["other"] === "string" ? group["other"] : undefined;
}

/** Interpolating `t`. Replaces the hand-rolled `t(k, f).replace('{n}', …)` chain. */
export function tf(
  key: string,
  params: Record<string, string | number>,
  fallback = "",
): string {
  return interpolate(t(key, fallback), params);
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
  fallback = "",
): string {
  const form = selectPluralForm(lookup(key), count, currentLang) ?? fallback;
  return interpolate(form, { n: count, ...params });
}

export function tArr(key: string, fallback: string[]): string[] {
  const val = key
    .split(".")
    .reduce<unknown>((o, k) => (o as Record<string, unknown>)?.[k], T);
  return Array.isArray(val) ? (val as string[]) : fallback;
}
