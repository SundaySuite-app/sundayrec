/**
 * Innstillingsprofilens rene halvdel: rekkefølgen, og hva et svar betyr.
 *
 * ## Hvorfor appen ikke lenger velger fila selv
 *
 * Før åpnet skallet lagre-/åpne-vinduet selv (dialog-pluginen i JS) og
 * sendte STIEN det svarte til bakenden. Men appen har ingen tilgangsliste per
 * kommando: et kompromittert webview kunne kalt eksporten med en hvilken som
 * helst sti uten å vise noe vindu — og skrevet en fil med innhold det selv
 * formet hvor som helst brukeren kan skrive (funn A1, `SECURITY.md`).
 *
 * Nå åpner Rust vinduet (`src-tauri/src/commands/settings.rs`), og ingen sti
 * krysser grensen. Skallet ber bare om at det skjer, og får tilbake:
 *
 *   • eksport: `true` (skrevet) eller `false` (avbrutt),
 *   • import: innstillingene som ble lagret, eller `null` (avbrutt).
 *
 * Et avbrutt vindu er et svar, ikke en feil, og skal ikke si noe.
 *
 * ## ⚠️ Spørsmålet kommer FØR vinduet nå
 *
 * Importen erstatter alt, så den spør først («Importere innstillinger?»). Før
 * kom spørsmålet mellom fila og importen; nå er vinduet og importen ett steg i
 * Rust, og det finnes ikke noe «mellom» å spørre i. Derfor spør
 * {@link runImport} før den ber om vinduet — et nei åpner ingenting.
 *
 * ## En fil som ikke er en profil, sier det med egne ord
 *
 * Bakenden avviser en fil som ikke er en innstillingsprofil
 * (`profile_not_settings`) eller som er alt for stor (`profile_too_large`) —
 * uten å endre noe. Før ble en feil valgt fil stille til STANDARDINNSTILLINGENE:
 * opptaksmappe, språk og tidsplan borte, og toasten sa «importert». De to
 * kodene får hver sin setning ({@link refusalOf}); alt annet får den generelle
 * med bakendens egne ord.
 */

import { errorCode } from "@lib/error-code-core";

/** De to avvisningene som har sin egen setning i kortet. */
export type ProfileRefusal = "notProfile" | "tooLarge";

/** Hvordan en eksport eller import endte — kortet gjør det om til en toast. */
export type ProfileOutcome =
  | { kind: "cancelled" }
  | { kind: "done" }
  | { kind: "failed"; err: string; refusal: ProfileRefusal | null };

/** Den stabile koden bak en avvisning, som kortets egen setning — eller
 *  `null` når den ikke har noen. */
export function refusalOf(err: unknown): ProfileRefusal | null {
  switch (errorCode(err)) {
    case "profile_not_settings":
      return "notProfile";
    case "profile_too_large":
      return "tooLarge";
    default:
      return null;
  }
}

function failed(err: unknown): ProfileOutcome {
  return { kind: "failed", err: errText(err), refusal: refusalOf(err) };
}

/**
 * Ett profilvindu om gangen. Et dobbeltklikk på «Eksporter» eller
 * «Importer» ba før om to native vinduer etter hverandre; nå gjør det andre
 * trykket ingenting så lenge det første pågår — spørsmålet før importen
 * medregnet. Én port for begge knappene: to vinduer samtidig er like
 * forvirrende uansett hvilke to det er.
 */
export function oneAtATime(): (task: () => Promise<void>) => Promise<void> {
  let busy = false;
  return async (task) => {
    if (busy) return;
    busy = true;
    try {
      await task();
    } finally {
      busy = false;
    }
  };
}

/**
 * Eksporten: be Rust åpne lagre-vinduet og skrive fila. `false` fra bakenden
 * er et avbrutt vindu.
 */
export async function runExport(
  exportProfile: () => Promise<boolean>,
): Promise<ProfileOutcome> {
  try {
    return (await exportProfile()) ? { kind: "done" } : { kind: "cancelled" };
  } catch (err) {
    return failed(err);
  }
}

/** Det importen trenger utenfra — injisert så rekkefølgen kan testes. */
export interface ImportDeps {
  /** «Importere innstillinger?» — `true` er ja. */
  confirm: () => Promise<boolean>;
  /** Rust åpner åpne-vinduet og importerer; `null` er et avbrutt vindu. */
  importProfile: () => Promise<unknown>;
  /** Les innstillingene inn på nytt gjennom den vanlige veien. */
  rehydrate: () => Promise<void>;
}

/**
 * Importen: spør, be Rust åpne vinduet og importere, og les så alt inn på
 * nytt. Et nei på spørsmålet og et avbrutt vindu er begge `cancelled`.
 *
 * Innlesingen etterpå går gjennom `hydrateSettings`, ikke returverdien:
 * signalene er det ene stedet skjermen leser fra, og en import som bare
 * oppdaterte returverdien ville latt hver åpne skjerm vise det gamle.
 */
export async function runImport(deps: ImportDeps): Promise<ProfileOutcome> {
  if (!(await deps.confirm())) return { kind: "cancelled" };
  try {
    const stored = await deps.importProfile();
    if (stored === null || stored === undefined) return { kind: "cancelled" };
    await deps.rehydrate();
    return { kind: "done" };
  } catch (err) {
    return failed(err);
  }
}

/**
 * Feilteksten en bruker faktisk kan gi videre. «[object Object]» er ikke en —
 * og det var det en avvist kommando ga før: Rusts `AppError` kommer over
 * grensen som `{ code, message }`, ikke som en `Error`.
 */
export function errText(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (err && typeof err === "object") {
    const { message, code } = err as { message?: unknown; code?: unknown };
    if (typeof message === "string" && message) return message;
    if (typeof code === "string" && code) return code;
  }
  return String(err);
}
