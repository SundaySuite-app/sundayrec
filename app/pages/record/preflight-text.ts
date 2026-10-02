/**
 * Forhåndssjekk-radens setning — ett funn, på brukerens språk.
 *
 * Skilt ut av `RecordPage.tsx` for at oppslaget kan prøves uten en DOM
 * (`preflight-text.test.ts`): det er her «hvilken setning står det?» avgjøres,
 * og avgjørelsen er nøyaktig den slags som ser riktig ut i en komponent og er
 * feil for ett funn av syv.
 */

import type { PreflightFinding } from "@legacy/bindings/PreflightFinding";

import { tDyn } from "../../i18n";
import { interpolate } from "../../state/backend-warning";

/**
 * Ett funn, som setning (F2-I18N-R2).
 *
 * `PreflightFinding.message` er motorens EGEN formulering — engelsk siden
 * F2-I18N-R2, fordi Rust-prosa er en reserve og ikke appens stemme. Den vises
 * bare for et funn UTEN kode, og det er nøyaktig de tre `buildHealthFindings`
 * lager selv: de er allerede skrevet på brukerens språk der.
 *
 * Har funnet en kode, er katalogen fasiten. `{gb}` og `{device}` fylles med
 * motorens egne `params` — tallet er et FAKTUM målt i det øyeblikket sjekken
 * kjørte, og skallets egen diskmåling er en annen måling til en annen tid.
 *
 * ## Spesialopptakets egen lydenhet
 *
 * `deviceMissing` peker på «lydenheten som er valgt i innstillingene», som er
 * FEIL enhet å lete etter når det er spesialopptakets egen som mangler.
 * Planleggeren sender da `specialDeviceMissing`, med enhetens navn som DATA i
 * `params.device` — samme kode slås opp i OS-varselet
 * (`AlertText::PreflightSpecialDeviceMissing`), med de samme ordene, så de to
 * flatene ikke kan si to ting. Navnet leses aldri ut av `message`: en kode skal
 * slås opp, ikke en setning parses.
 */
export function preflightText(f: PreflightFinding): string {
  if (!f.code) return f.message;
  return interpolate(tDyn("status.preflightCode", f.code), f.params);
}
