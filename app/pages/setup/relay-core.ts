/**
 * Reléets tilstandsmaskin — hva knappene under adressefeltet har lov til å si.
 *
 * ## Hvorfor en egen ren fil
 *
 * Dobbel opt-in har fire tilstander og ingen av dem er den samme skjermen:
 * ingenting påmeldt, venter på at noen trykker i innboksen, bekreftet, og
 * «adressen tar ikke imot e-post». Skrevet som `&&`-er inne i JSX blir det fire
 * uleste linjer, og den femte tilstanden — den ingen tenkte på — er en knapp
 * som lover noe bakenden vil avvise. `notify_relay.rs` avviser nemlig HELT
 * eksplisitt: «Bekreft» uten endepunkt er `relay_no_endpoint`, «Send en test»
 * uten bekreftelse er `relay_not_confirmed`. En knapp som kan trykkes bare for
 * å få den feilmeldingen er en knapp som lyver.
 *
 * Så: tabellen her, en test per rad, og NotifyPage maler svaret.
 *
 * ## Adressen er den PÅMELDTE, ikke den i feltet
 *
 * Feltet er et utkast med Lagre/Avbryt (`useDraftForm`), og abonnementet
 * gjelder den adressen bakenden faktisk fikk. De to kan stå fra hverandre i
 * fullt dagslys — noen skriver en ny adresse uten å lagre — og da er det
 * abonnementets adresse som beskriver tilstanden. Utkastet får bare bestemme
 * ÉN ting: om «Bekreft» er trykkbar, for å melde på noe som ikke er lagret er
 * å melde på noe brukeren ikke har sagt seg ferdig med.
 */

import type { RelaySubscriptionStatus } from "@legacy/bindings/RelaySubscriptionStatus";

/**
 * Endepunktets egen sperre mellom to bekreftelses-e-poster, speilet lokalt.
 *
 * Workeren svarer 429 innenfor den, og utboksen backer av og prøver igjen — så
 * et utålmodig klikk koster en ventetid, aldri påmeldingen. Men en knapp som
 * ser trykkbar ut og bare fører til en usynlig kø er en knapp som ikke svarer,
 * så skjermen holder samme frist og SIER hvor lenge det er igjen.
 */
export const RELAY_RESEND_COOLDOWN_MS = 10 * 60 * 1000;

/** Hvor abonnementet står, sett fra skjermen. */
export type RelayStep =
  /** Denne utgaven har ingen tjeneste å sende gjennom. */
  | "unavailable"
  /** Ingen adresse er meldt på fra denne maskinen. */
  | "none"
  /** Meldt på, venter på at noen trykker på lenken i e-posten. */
  | "pending"
  /** Bekreftet — reléet er en sendevei nå. */
  | "confirmed"
  /** Adressen avviser e-post (retur, blokkering). */
  | "suppressed";

/** Hvorfor «Bekreft e-postadressen» er av. `null` = trykkbar. */
export type ConfirmBlock =
  /** Ingen tjeneste å melde seg på. */
  | "noEndpoint"
  /** Feltet er tomt, eller det står noe uslagret i det. */
  | "unsaved"
  /** Den lagrede adressen ER den som avviste e-posten. */
  | "sameSuppressed";

export interface RelayViewInput {
  /** `relayFacts`. `null` = ikke lest ennå. */
  facts: RelaySubscriptionStatus | null;
  /** Adressen som er LAGRET — ikke utkastet i feltet. */
  savedAddress: string;
  /** Utkastet skiller seg fra det lagrede. */
  dirty: boolean;
  /** Da «Send på nytt» sist ble trykket i denne økten. `null` = aldri. */
  lastResendAt: number | null;
  /** Nå, i unix-ms. */
  now: number;
}

export interface RelayView {
  step: RelayStep;
  /** Adressen tilstanden handler om. Tom når ingenting er påmeldt. */
  address: string;
  showConfirm: boolean;
  /** `null` når knappen kan trykkes. */
  confirmBlock: ConfirmBlock | null;
  showResend: boolean;
  /** Millisekunder igjen av sperren. `0` = kan sendes nå. */
  resendWaitMs: number;
  showUnsubscribe: boolean;
  /** Noe ligger i utboksen og venter på nett. */
  queued: boolean;
  /** Reléet er en sendevei akkurat nå (bekreftet, og bygget har endepunkt). */
  transport: boolean;
}

/** Bakenden folder adressen til små bokstaver (`normalize_address`), så
 *  sammenligningen må gjøre det samme — ellers ser «Ola@» og «ola@» ut som to
 *  forskjellige adresser og skjermen tilbyr en ny påmelding for ingenting. */
function sameAddress(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

/**
 * Hele tilstandsmaskinen, som data.
 *
 * `facts === null` (ikke lest ennå) leses som «ingenting påmeldt», av samme
 * grunn som `notifyGateStatus` leser `null` som `ok`: skjermen skal ikke stå
 * med en sperret knapp i det halvsekundet det tar å spørre bakenden. Det er
 * dessuten den ufarlige gjetningen — trykker noen i det vinduet, er handlingen
 * «meld på adressen», som er nøyaktig det de ba om.
 */
export function relayView(input: RelayViewInput): RelayView {
  const { facts, savedAddress, dirty, lastResendAt, now } = input;

  const address = facts?.address ?? "";
  const step: RelayStep =
    facts === null
      ? "none"
      : // Uten endepunkt kan ingenting sendes, uansett hva den lokale raden
        // sier. En nedgradert build som fortsatt husker et abonnement skal si
        // «ikke tilgjengelig», ikke «bekreftet».
        !facts.endpointBuilt
        ? "unavailable"
        : (facts.state ?? "none");

  const enrolledIsSaved = sameAddress(savedAddress, address);
  const showConfirm =
    step !== "confirmed" && !(step === "pending" && enrolledIsSaved);

  let confirmBlock: ConfirmBlock | null = null;
  if (step === "unavailable") confirmBlock = "noEndpoint";
  else if (!savedAddress.trim() || dirty) confirmBlock = "unsaved";
  else if (step === "suppressed" && enrolledIsSaved)
    confirmBlock = "sameSuppressed";

  const showResend = step === "pending" && enrolledIsSaved;
  const resendWaitMs =
    lastResendAt === null
      ? 0
      : Math.max(0, lastResendAt + RELAY_RESEND_COOLDOWN_MS - now);

  return {
    step,
    address,
    showConfirm,
    confirmBlock: showConfirm ? confirmBlock : null,
    showResend,
    resendWaitMs: showResend ? resendWaitMs : 0,
    showUnsubscribe: step === "confirmed" || step === "suppressed",
    queued: (facts?.queued ?? 0) > 0,
    transport: step === "confirmed",
  };
}

/** Sperren i HELE minutter, alltid minst 1 så lenge det er noe igjen — «om 0
 *  min» er en knapp som burde vært trykkbar. */
export function resendWaitMinutes(waitMs: number): number {
  return Math.max(1, Math.ceil(waitMs / 60_000));
}

/**
 * Er reléet en sendevei akkurat nå? `null` = ikke lest ennå.
 *
 * Den ene biten av `relayView` som flatene UTENFOR denne siden trenger —
 * kontrollrommets kort 5 og førstegangs-sjekklisten spør begge om det finnes
 * en vei ut, og ingen av dem har et adressefelt å fôre resten av tabellen med.
 * Egen inngang og ikke et håndlagt `state === "confirmed"` på hvert kallsted,
 * fordi den skjulte halvdelen av regelen er den som blir glemt: en build uten
 * endepunkt kan fortsatt huske et bekreftet abonnement.
 */
export function relayTransport(
  facts: RelaySubscriptionStatus | null,
): boolean | null {
  if (facts === null) return null;
  return facts.endpointBuilt && facts.state === "confirmed";
}
