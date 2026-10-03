/**
 * Innstillingsprofilens rene halvdel: rekkefølgen, og hva et svar betyr.
 *
 * ## Hvorfor appen ikke lenger velger fila selv
 *
 * Før åpnet skallet lagre-/åpne-vinduet (`@tauri-apps/plugin-dialog`) og
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
 */

/** Hvordan en eksport eller import endte — kortet gjør det om til en toast. */
export type ProfileOutcome =
  { kind: "cancelled" } | { kind: "done" } | { kind: "failed"; err: string };

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
    return { kind: "failed", err: errText(err) };
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
    return { kind: "failed", err: errText(err) };
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
