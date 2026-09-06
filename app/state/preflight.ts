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
 * ⚠️ `buildHealthFindings` svarer fortsatt med TEKST, så et språkbytte etterpå
 * oversetter ikke DE tre funnene. Det er arvet fra legacy, og det riktige
 * stedet å løse det er å la den svare med data i stedet for setninger — samme
 * grep som `decisions-core`. Ikke i P2.
 *
 * ## BAKENDENS funn er data (F2-W9 → F2-I18N-R2)
 *
 * F2-W9 ga det første funnet en stabil `code` og oversatte det HER, ved
 * mottak, med én `if` i `localizeBackendFinding`. F2-I18N-R2 ga hvert eneste
 * bakend-funn en `PreflightCode`, og flyttet oppslaget til der raden TEGNES
 * (`preflightText` i `RecordPage`, mot `status.preflightCode.<kode>`).
 *
 * Det er ikke bare ryddigere — det fikser ⚠️-en over for bakendens halvdel: en
 * oversettelse gjort ved mottak fryser språket den hadde da, så en frivillig
 * som bytter språk etterpå blir stående med det gamle. Et oppslag ved render
 * kan ikke gjøre det. Rust-teksten (`message`) er ENGELSK reserve for et funn
 * uten kode — og det er bare de tre `buildHealthFindings` lager selv.
 */

import { buildHealthFindings } from "@lib/status/health-findings";
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
    const findings = [...health, ...(result?.findings ?? [])];
    // Bare når det FAKTISK er noe. Et tomt skriv ville tømt et varsel
    // planleggeren nettopp la igjen og brukeren ennå ikke har sett.
    if (findings.length > 0) setPreflightFindings(findings);
  } catch (err) {
    console.warn("[preflight] den stille sjekken kom ikke gjennom:", err);
  }
}
