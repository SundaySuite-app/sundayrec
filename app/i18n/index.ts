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
  tArr as legacyTArr,
  tf as legacyTf,
  tn as legacyTn,
} from "@lib/i18n";

/** De sju katalogene som finnes i `legacy/locales`. */
export type Locale = "no" | "en" | "sv" | "da" | "de" | "fr" | "pl";

/**
 * De samme sju, som en LISTE.
 *
 * Typen alene kan ikke gås gjennom av en test, og noen ting må gjelde for alle
 * sju og ikke bare for de aktive — navnet på språket, for eksempel. Uten det
 * hadde `app.language`-subtreet bare de to aktive kodene i katalogen, og
 * `tDyn('app.language', 'de')` ville kastet i DEV og rendret en TOM etikett i
 * prod den dagen et pauset språk ble tatt i bruk. En tabelltest over denne
 * lista er det som fanger hullet før brukeren gjør det.
 */
export const ALL_LOCALES: readonly Locale[] = [
  "no",
  "en",
  "sv",
  "da",
  "de",
  "fr",
  "pl",
];

/**
 * Språkene `app/` faktisk tilbyr i språkvelgeren gjennom redesignet.
 *
 * De fem andre er PAUSET, ikke fjernet: katalogene ligger der, legacy-skallet
 * bruker dem, og fase B tar dem opp igjen. Begrunnelsen og listen over hvilke
 * nøkler pausen gjelder står i `legacy/locales/parity.test.ts`
 * (`PAUSED_LOCALES` / `PAUSED_KEYS`) — ett sted, ved siden av testen som
 * håndhever den.
 */
export const ACTIVE_LOCALES: readonly Locale[] = ["no", "en"];

/**
 * Hvilket språk skallet skal starte på, gitt det som står lagret.
 *
 * Legacy gjør `settings.language ?? 'no'` og er ferdig. Det kan ikke `app/`
 * gjøre så lenge de fem andre språkene er PAUSET: en bruker som satte tysk i
 * det gamle skallet ville fått et nytt skall der de redesignede tekstene er
 * TOMME — `t()` svarer med tom streng for en nøkkel som ikke finnes, så
 * skjermen ville sett halvferdig ut uten å si hvorfor.
 *
 * Så vi velger et aktivt språk i stedet, og velger det nærmeste:
 * svensk og dansk går til norsk (nabospråk, og det er den nordiske
 * menighetsvirkeligheten), resten går til engelsk. Ingenting skrives til
 * innstillingene — det lagrede valget står, og fase B tar det i bruk igjen.
 */
export function resolveStartupLocale(stored: string | null): Locale {
  if (!stored) return "no";
  if ((ACTIVE_LOCALES as readonly string[]).includes(stored)) {
    return stored as Locale;
  }
  return stored === "sv" || stored === "da" ? "no" : "en";
}

/**
 * Språket som gjelder nå. Les det i en komponent for å abonnere på bytte;
 * skriv det aldri direkte — `setLocale` er den ene veien, fordi katalogen må
 * være lastet først.
 */
export const locale = signal<Locale>(currentLang as Locale);

/** Tom liste for `tArr`. Frossen og delt: den returneres aldri til noen som
 *  har lov til å endre den, og en ny array per kall ville gitt et nytt
 *  identitetsbytte i hver render. */
const NO_ITEMS: readonly string[] = Object.freeze([]);

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

/** Oversett en nøkkel som er en LISTE i katalogen. */
export function tArr(key: string): string[] {
  track();
  return legacyTArr(key, NO_ITEMS as string[]);
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
