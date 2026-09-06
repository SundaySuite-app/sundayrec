/**
 * Den stille forhåndssjekken — én gang per oppstart.
 *
 * ## Hvorfor den finnes
 *
 * `scheduler://preflight` fyrer 30 minutter FØR et planlagt opptak. Det er for
 * sent for den frivillige som åpner appen fem minutter før gudstjenesten, og
 * det skjer aldri i det hele tatt for den som tar opp manuelt. Så: samme sjekk,
 * kjørt når appen åpnes, med de samme funnene.
 *
 * ## Og hvorfor de to helse-probene er med
 *
 * `media_permissions` og `ffmpeg_health` er to bakendkommandoer legacy hadde
 * uten en eneste kaller. Den som betyr mest er mikrofonen: en avslått mikrofon
 * får enhetsåpningen til å feile med en generisk feil, og brukeren fikk beskjed
 * om at enheten manglet når det i virkeligheten var macOS som sa nei. Svaret
 * fantes — AVFoundation visste det — det nådde bare aldri en skjerm.
 *
 * `buildHealthFindings` er GJENBRUKT, ikke portet
 * (`@lib/status/health-findings`): ordlyden og avgjørelsen om hva som er verdt
 * å melde er allerede tabelltestet der, og en kopi ville vært to steder «denied
 * er blokkert, notDetermined er det ikke» kunne begynne å bety forskjellige
 * ting. Funnene legges FORAN `run_preflight` sine, fordi en tillatelse OS-et
 * nekter slår en nesten full disk.
 *
 * ## Én gang per oppstart, ikke per sidebesøk
 *
 * Samme regel som legacy `runSilentPreflightOnce`. Å male de samme gule
 * punktene hver gang noen navigerer tilbake til OPPTAK er hvordan gult slutter
 * å bety noe.
 *
 * ⚠️ Funnene er TEKST når de er bygget, så et språkbytte etterpå oversetter
 * dem ikke. Det er arvet fra legacy, og det riktige stedet å løse det er å la
 * `buildHealthFindings` svare med data i stedet for setninger — samme grep som
 * `decisions-core`. Ikke i P2.
 *
 * ## Unntaket: funn med en `code` (F-W9)
 *
 * `PreflightFinding.message` fra bakenden er norsk for hvert funn som fantes
 * FØR F-W9 — det er gjelden avsnittet over beskriver. Et funn lagt til SIDEN
 * (`sundayrec_core::preflight::code`) bærer i stedet en stabil `code`, og
 * Rust-teksten er en ENGELSK reserve for en gammel klientversjon som ikke
 * kjenner koden — se `crates/sundayrec-core/src/preflight.rs` og
 * `scripts/check-rust-norwegian.mjs`. `localizeBackendFinding`
 * (`@lib/status/next-recording-core`, delt med `next-recording.ts` — SAMME
 * bakend-funn når `scheduler://preflight` fyrer live) er broen: den erstatter
 * meldingen med appens egen `preflight.*`-nøkkel når koden er kjent, og lar
 * alt annet (inkludert hvert funn uten `code`) stå urørt.
 */

import { buildHealthFindings } from "@lib/status/health-findings";
import { localizeBackendFinding } from "@lib/status/next-recording-core";
import type { PreflightFinding } from "@legacy/bindings/PreflightFinding";

import { t } from "../i18n";
import { setPreflightFindings } from "./next-recording";
import { settings } from "./settings";

let hasRun = false;

/** Test-krok: glem at sjekken har kjørt. */
export function __resetSilentPreflight(): void {
  hasRun = false;
}

/** Tillatelsene og sidecar-helsen, som forhåndssjekk-funn. Best effort — en
 *  probe vi ikke fikk kjørt legger ingenting til i stedet for å blokkere. */
export async function collectHealthFindings(): Promise<PreflightFinding[]> {
  const [permissions, ffmpeg] = await Promise.all([
    window.api.mediaPermissions?.().catch(() => null) ?? null,
    window.api.ffmpegHealth?.().catch(() => null) ?? null,
  ]);
  return buildHealthFindings({
    permissions,
    ffmpeg,
    videoEnabled: settings.peek().videoEnabled === true,
    t,
  });
}

/** Kjør sjekken hvis den ikke har kjørt. Trygg å kalle fra en `useEffect`. */
export async function runSilentPreflightOnce(): Promise<void> {
  if (hasRun) return;
  hasRun = true;
  try {
    const [health, result] = await Promise.all([
      collectHealthFindings(),
      // Kastet, akkurat som legacy gjør det: den omgivende typen i
      // `main.ts` beskriver `category` som `string`, mens bakenden svarer med
      // den genererte `PreflightCategory`. Verdien er den samme; det er typen
      // som er for løs.
      window.api.runPreflight() as Promise<{ findings?: PreflightFinding[] }>,
    ]);
    // Only the BACKEND half needs re-localizing — `health` was already built
    // with the app's own `t()` by `collectHealthFindings` above.
    const backend = (result?.findings ?? []).map((f) =>
      localizeBackendFinding(f, t),
    );
    const findings = [...health, ...backend];
    // Bare når det FAKTISK er noe. Et tomt skriv ville tømt et varsel
    // planleggeren nettopp la igjen og brukeren ennå ikke har sett.
    if (findings.length > 0) setPreflightFindings(findings);
  } catch (err) {
    console.warn("[preflight] den stille sjekken kom ikke gjennom:", err);
  }
}
