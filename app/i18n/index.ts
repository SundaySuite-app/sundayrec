/**
 * Reaktiv i18n for `app/`.
 *
 * ## Hva som er nytt, og hvorfor
 *
 * Legacy-skallet oversetter ved å SKRIVE I DOM-EN: hver node bærer et
 * `data-i18n`-attributt, og `applyTranslations()` går gjennom dokumentet og
 * setter `textContent` på nytt ved språkbytte. Det virker i et tre ingen andre
 * rører — men i et Preact-tre ville neste render strøket skrivingen, stille,
 * og bare noen ganger. `scripts/check-i18n-hardcoded-tsx.mjs` forbyr derfor
 * `data-i18n` i `app/` helt.
 *
 * Her er språket i stedet et SIGNAL. En komponent som kaller `t()` leser
 * `locale.value` på veien, og abonnerer dermed uten å vite at den gjør det;
 * når signalet endres, rendrer akkurat de komponentene på nytt. Ingen
 * `onLocaleApplied`-hook, ingen «live-flater må repaintes etter
 * data-i18n-passet»-dans — den finnes ikke å gjøre feil.
 *
 * ## Rekkefølgen som ikke kan snus
 *
 * `setLocale` laster KATALOGEN først og setter signalet ETTERPÅ. Snudd ville
 * det gitt ett render med det nye språket satt og den gamle katalogen lastet —
 * altså norsk tekst i en engelsk app, i én frame, hver gang noen bytter språk.
 * Det er nøyaktig den slags feil ingen rapporterer og ingen finner.
 *
 * ## Ingen fallback-argumenter
 *
 * `t(key)`, ikke `t(key, 'Norsk reservetekst')`. En fallback gjør en manglende
 * nøkkel usynlig: UI-et leser riktig på norsk og går stille uoversatt ut i de
 * andre språkene. Håndhevet tre steder — ESLint (aritet), aritetssjekken i
 * `scripts/check-i18n-keys.mjs`, og her, ved at argumentet ikke finnes.
 */

import { signal } from "@preact/signals";
import {
  currentLang,
  loadLocaleCatalogue,
  t as legacyT,
  tf as legacyTf,
  tn as legacyTn,
} from "@lib/i18n";

/** De to katalogene som finnes i `legacy/locales` (F4: bare norsk og engelsk). */
export type Locale = "no" | "en";

/**
 * De samme to, som en LISTE: rekkefølgen er den valgboksen viser, og det er
 * den en tabelltest går gjennom.
 */
export const ALL_LOCALES: readonly Locale[] = ["no", "en"];

/**
 * Systemregelen: et norsk system (bokmål, nynorsk eller «no») gir norsk, alt annet gir
 * engelsk. Leser den første av `navigator.languages`, ellers `navigator.language`. `typeof
 * navigator` fordi enhetsgaten kjører i node, der svaret er engelsk.
 */
export function systemLocale(
  languages: readonly string[] | undefined = typeof navigator === "undefined"
    ? undefined
    : navigator.languages?.length
      ? navigator.languages
      : [navigator.language],
): Locale {
  const first = languages?.[0] ?? "";
  return /^(nb|nn|no)(-|_|$)/i.test(first) ? "no" : "en";
}

/**
 * Hvilket språk skallet skal starte på, gitt det som står lagret.
 *
 * `no` og `en` brukes som de er. Ingenting lagret (`null`, den ekte førstegangsinstallasjonen)
 * er norsk, som alltid. Et lagret språk som ikke finnes lenger — sv, da, de, fr, pl fra før
 * F4, eller en korrupt eller håndredigert rad — faller STILLE tilbake på systemregelen
 * ([`systemLocale`]): norsk på et norsk system, ellers engelsk. Ingen feilmelding og ingen
 * skriving tilbake til basen: den lagrede verdien står, og blir bare ikke brukt.
 */
export function resolveStartupLocale(
  stored: string | null,
  system: Locale = systemLocale(),
): Locale {
  if (!stored) return "no";
  if ((ALL_LOCALES as readonly string[]).includes(stored)) {
    return stored as Locale;
  }
  return system;
}

/**
 * Språket som gjelder nå. Les det i en komponent for å abonnere på bytte;
 * skriv det aldri direkte — `setLocale` er den ene veien, fordi katalogen må
 * være lastet først.
 */
export const locale = signal<Locale>(currentLang as Locale);

/**
 * Les signalet uten å bruke verdien.
 *
 * Det er ABONNEMENTET som er poenget: `@preact/signals` sporer lesninger som
 * skjer mens en komponent rendrer, uansett hvor dypt i kallstakken de skjer.
 * Selve teksten kommer fra `@lib/i18n`s modulnivå-katalog, som `setLocale`
 * allerede har byttet før signalet fikk sin nye verdi.
 */
function track(): void {
  void locale.value;
}

/** Oversett en nøkkel. */
export function t(key: string): string {
  track();
  return legacyT(key);
}

/** Oversett en nøkkel med `{navn}`-innsettinger. */
export function tf(
  key: string,
  params: Record<string, string | number>,
): string {
  track();
  return legacyTf(key, params);
}

/** Oversett en tellende nøkkel. `{n}` er bundet til `count`. */
export function tn(
  key: string,
  count: number,
  params: Record<string, string | number> = {},
): string {
  track();
  return legacyTn(key, count, params);
}

/**
 * Den ENE hjelperen for en dynamisk nøkkel: `tDyn('app.page', route.page)`.
 *
 * Én, ikke null og ikke mange. Null ville betydd at hver dynamisk nøkkel ble
 * skrevet som en template-streng inne i `t()`, og da kan ingen gate se hva som
 * slås opp. Mange ville betydd like mange former å sjekke.
 *
 * Prefikset MÅ være en literal: `scripts/check-i18n-keys.mjs` slår det opp og
 * krever at det peker på et ikke-tomt objekt-subtre i både no.json og en.json.
 * Suffikset er den halvdelen som er dynamisk, og det er den gaten ikke kan
 * kjenne — derfor kaster vi i DEV når oppslaget bommer, i stedet for å
 * returnere tom tekst. En tom etikett i UI er nettopp den feilen som overlever
 * en hel testrunde fordi den ser ut som «denne er visst tom».
 */
export function tDyn(prefix: string, suffix: string): string {
  track();
  const value = legacyT(`${prefix}.${suffix}`);
  if (!value && import.meta.env.DEV) {
    throw new Error(
      `tDyn: «${prefix}.${suffix}» finnes ikke i katalogen for «${locale.value}». ` +
        "Prefikset er sjekket av check-i18n-keys.mjs; suffikset er det ikke.",
    );
  }
  return value;
}

/**
 * Bytt språk: last katalogen, så flipp signalet.
 *
 * Alltid i den rekkefølgen (se toppen av fila). Løser alltid — en ukjent kode
 * eller en feilet import ender på `no`, akkurat som i legacy-skallet, og
 * `currentLang` er fasiten på hva som faktisk ble aktivert.
 */
export async function setLocale(lang: Locale): Promise<void> {
  await loadLocaleCatalogue(lang);
  locale.value = currentLang as Locale;
  // Menylinje-ikonets etiketter rendres i Rust og kan ikke lese UI-språket.
  // Samme push som `loadLocale` gjør for legacy-skallet — dette skallet kaller
  // ikke `loadLocale`, så det ville ellers vært det ene stedet hvor et
  // språkbytte lot ikonet stå på gammelt språk. `typeof window` fordi
  // enhetsgaten kjører i node.
  if (typeof window !== "undefined") {
    void window.api?.traySetLanguage?.(locale.value);
  }
}
