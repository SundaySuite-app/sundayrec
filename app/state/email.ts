/**
 * Kan denne maskinen faktisk sende en e-post?
 *
 * Tre uavhengige fakta må stemme, og ingen av dem er synlige i
 * innstillingsbasen alene:
 *
 *   1. `email_status` — er e-postveien BYGGET inn i denne utgaven? Sendingen
 *      ligger bak en cargo-feature, og en build uten den har ingen sendevei
 *      uansett hva brukeren skriver.
 *   2. SMTP-vert og brukernavn i innstillingene.
 *   3. Et passord i OS-nøkkelringen (`email_has_smtp_password`) — passordet er
 *      ikke et `Settings`-felt og kan derfor aldri ri med en lagring.
 *
 * Avgjørelsen selv er `hasEmailTransport` i `@lib/ui/feature-gate-core`, som
 * legacy-skallet også bruker. Denne fila er bare de tre lesningene og signalet
 * de havner i, slik at nivå 1 kan si sant om spørsmål 5 uten å gjette.
 *
 * ## Hvorfor en generasjonsteller
 *
 * Flere flater ber om en oppfriskning (siden åpnes, bryteren snus, adressen
 * skrives). To overlappende kjøringer som hver maler halvparten ender med å
 * beskrive en tilstand som aldri fantes — det skjedde på ekte i legacy
 * («passordet er ikke lagret» ved siden av «sendeveien er klar»). Så: les alt
 * først, kast resultatet hvis en nyere kjøring har startet, og skriv én gang.
 */

import { signal } from "@preact/signals";
import { hasEmailTransport, type EmailFacts } from "@lib/ui/feature-gate-core";

import { settings } from "./settings";

/** Det vi vet om sendeveien. `null` = ikke lest ennå. */
export const emailFacts = signal<EmailFacts | null>(null);

/** Finnes det en vei ut for en e-post? `null` = ikke lest ennå. */
export function emailTransport(): boolean | null {
  const facts = emailFacts.value;
  return facts === null ? null : hasEmailTransport(facts);
}

let seq = 0;

/**
 * Les de tre fakta på nytt.
 *
 * `typedPassword` er passordet som står i feltet akkurat nå, for flatene som
 * har et slikt felt: bakenden foretrekker det over nøkkelringens
 * (`resolve_smtp_password`), så et nettopp innskrevet passord ER en sendevei.
 * Nivå 1 har ikke feltet og sender ingenting.
 */
export async function refreshEmailFacts(typedPassword = ""): Promise<void> {
  const mine = ++seq;
  const [stored, status] = await Promise.all([
    window.api.emailHasSmtpPassword().catch(() => false),
    // En feilet statussjekk er ikke en sendevei. Fall tilbake på det
    // pessimistiske svaret, ikke på det forrige.
    window.api.emailStatus().catch(() => ({ featureBuilt: false })),
  ]);
  if (mine !== seq) return;

  const s = settings.peek();
  emailFacts.value = {
    featureBuilt: !!status.featureBuilt,
    smtpConfigured:
      !!(s.emailSmtp ?? "").trim() && !!(s.emailSmtpUser ?? "").trim(),
    smtpPasswordAvailable: stored || !!typedPassword.trim(),
  };
}
