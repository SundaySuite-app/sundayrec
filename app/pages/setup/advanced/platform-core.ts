/**
 * Hvilket operativsystem kjører dette — som en ren avgjørelse.
 *
 * ## Hvorfor ikke bare UA-strengen
 *
 * Legacy avgjør dette med `navigator.userAgent.toLowerCase().includes('win')`
 * (`api-shim.ts`s `platform`). Det virker, men det er den skjøreste av alle
 * kilder: UA-strengen er en tekst nettleseren har lov til å finne på, den
 * inneholder produktnavn som TILFELDIGVIS har «win» eller «mac» i seg, og
 * SundayRecs egen WKWebView sender allerede en UA uten `Safari`-token — det er
 * nettopp derfor SundayEdits E5 målte en 42× regresjon som var usynlig i
 * Chromium. En gate som bestemmer om Windows-only-brytere finnes bør ikke hvile
 * på den alene.
 *
 * Så: tre kilder, i rekkefølge etter hvor mye de faktisk VET.
 *
 *   1. `navigator.userAgentData.platform` — den strukturerte verdien, satt av
 *      motoren, ikke satt sammen av en streng. Finnes i Chromium; ikke i
 *      WebKit.
 *   2. `navigator.platform` — «MacIntel», «Win32», «Linux x86_64». Formelt
 *      utdatert, men den ER satt i WKWebView, og den er en kort verdi fra et
 *      lite sett i stedet for en fritekstlinje.
 *   3. UA-strengen, som legacy — siste utvei, aldri den første.
 *
 * Ren og tabelltestet, fordi svaret bestemmer om en kontroll er der eller
 * ikke: en Windows-bryter som stille dukker opp på macOS skriver en
 * innstilling ingen leser der, og en som stille forsvinner på Windows tar bort
 * den ene nødutgangen en hakkete rigg har.
 */

/** Det UI-et trenger å skille mellom. */
export type Os = "mac" | "win" | "linux" | "other";

/** De feltene vi leser. Et objekt og ikke `Navigator`, så testen slipper DOM. */
export interface PlatformFacts {
  /** `navigator.userAgentData?.platform` */
  uaDataPlatform?: string | null;
  /** `navigator.platform` */
  platform?: string | null;
  /** `navigator.userAgent` */
  userAgent?: string | null;
}

/**
 * De STRUKTURERTE verdiene: `navigator.userAgentData.platform` og
 * `navigator.platform`. Korte ord fra et lite sett («MacIntel», «Win32»,
 * «Linux x86_64», «Windows»), så en delstreng er trygg nok her.
 */
function classifyShort(raw: string | null | undefined): Os | null {
  const value = (raw ?? "").toLowerCase();
  if (!value) return null;
  if (value.includes("mac") || value.includes("darwin")) return "mac";
  if (value.includes("win")) return "win";
  if (value.includes("linux") || value.includes("x11")) return "linux";
  return null;
}

/**
 * UA-strengen, som er en HEL SETNING og derfor ikke tåler den samme
 * delstrengjakten: «Winamp» inneholder «win», og et produktnavn er ikke et
 * operativsystem. Her leter vi bare etter de lange formene motorene faktisk
 * skriver, i den rekkefølgen som gjør «X11; CrOS» til Linux og ikke til noe
 * annet.
 */
function classifyUa(raw: string | null | undefined): Os | null {
  const value = (raw ?? "").toLowerCase();
  if (!value) return null;
  if (value.includes("macintosh") || value.includes("mac os")) return "mac";
  if (value.includes("windows")) return "win";
  if (value.includes("linux") || value.includes("x11")) return "linux";
  return null;
}

/**
 * Operativsystemet, fra den mest pålitelige kilden som svarer.
 *
 * `other` når ingen av dem sier noe. `other` er IKKE «antakelig Windows»: en
 * gate som gjetter feil vei her viser en bryter som ikke gjør noe.
 */
export function detectOs(facts: PlatformFacts): Os {
  return (
    classifyShort(facts.uaDataPlatform) ??
    classifyShort(facts.platform) ??
    classifyUa(facts.userAgent) ??
    "other"
  );
}

/** Det ekte vinduet, lest når noen spør. Ingen `window`-oppslag under
 *  modullast — node-gaten importerer denne fila. */
export function currentOs(): Os {
  if (typeof navigator === "undefined") return "other";
  const withData = navigator as Navigator & {
    userAgentData?: { platform?: string };
  };
  return detectOs({
    uaDataPlatform: withData.userAgentData?.platform ?? null,
    platform: navigator.platform ?? null,
    userAgent: navigator.userAgent ?? null,
  });
}
