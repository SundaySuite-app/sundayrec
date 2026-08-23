/**
 * Innstillingene som signal — `app/`s ene kopi av sannheten.
 *
 * ## Hvorfor signaler og ikke en butikk med abonnenter
 *
 * Legacy-skallet har `state.ts`: en mutabel `settings`-variabel og en debounced
 * lagring. Hver flate leser variabelen når den tegnes, og ingenting sier fra
 * når den endres — derfor finnes `applyXSettingsToUI()`, `loadSettings()` på
 * `window`, og `resyncBoundSettings()`, som alle er navn på det samme
 * problemet: to steder som hver for seg tror de vet hva verdien er.
 *
 * Et signal på modulnivå er den nærmeste 1:1-porten av den variabelen som
 * finnes — samme ene forekomst, samme «importer og les» — men lesningen er
 * SPORET. En komponent som leser `settings.value` rendres på nytt når den
 * endres, uten at noen kaller den, og uten at noen kan glemme å kalle den.
 *
 * At det bor på modulnivå (og ikke i en context) er også med vilje: mye av
 * appen lever UTENFOR rendertreet — VU-målerens rAF-løkke, opptaks-events,
 * pre-roll-forliket. De kan lese `settings.peek()` eller sette opp en `effect()`
 * uten å måtte være en komponent, og uten at vi må bygge en bro mellom «inne i
 * React-treet» og «utenfor».
 *
 * ## Aldri stille defaults
 *
 * `api-shim`s `getSettings` svarer med SETTINGS_DEFAULTS hvis `settings_get`
 * feiler, fordi UI-et må rendre noe. Det gjør at en ødelagt innstillingsbase
 * ser NØYAKTIG ut som en fabrikkny app — og en frivillig som «mistet alle
 * innstillingene» har ingen måte å vite at det var en lesefeil. Derfor spør vi
 * IPC-feilringen etterpå: registrerte shimmen en `settings_get`-feil, er
 * `hydrateError` satt, og S1b viser det som et banner.
 */

import { signal } from "@preact/signals";
import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";
import { SAVE_COALESCE_MS } from "@lib/ui/bind-setting-core";

import {
  IDLE_SAVE_TIMER,
  planFlush,
  planSave,
  payloadFor,
  type SaveTimerState,
} from "./settings-save-core";

/**
 * Den genererte ts-rs-typen, nådd gjennom den ene aliasen `app/` har lov til å
 * bruke. `SETTINGS_DEFAULTS` er annotert `: Settings`, så dette ER den typen —
 * og et felt lagt til (eller fjernet) i Rust-strukturen slår ut som en
 * kompileringsfeil her, akkurat som i legacy-skallet.
 */
export type Settings = typeof SETTINGS_DEFAULTS;

/** Innstillingene som gjelder nå. Skriv aldri direkte — bruk `patchSettings`. */
export const settings = signal<Settings>({ ...SETTINGS_DEFAULTS });

/** Har vi lest fra basen ennå? Falsk betyr «dette er defaults, ikke svaret». */
export const hydrated = signal(false);

/**
 * Hvorfor lesningen mislyktes, som en katalognøkkel UNDER `error.` — ellers
 * `null`.
 *
 * En nøkkel og ikke en ferdig streng, så banneret oversettes med språket som
 * gjelder når det vises. Og et SUFFIKS og ikke en hel nøkkel, fordi flaten da
 * skriver `tDyn("error", hydrateError.value)`: prefikset er en literal som
 * `check-i18n-keys.mjs` slår opp og krever finnes i både no.json og en.json.
 * En variabel med hele nøkkelen i ville vært usynlig for gaten.
 */
export type HydrateErrorKey = "settingsLoadFailed";
export const hydrateError = signal<HydrateErrorKey | null>(null);

/** Feilringen shimmen fyller. Tom liste når det ikke finnes noen shim (test). */
function ipcFailedFor(cmd: string): boolean {
  const failures = window.api?.getRecentIpcFailures?.() ?? [];
  return failures.some((f) => f.cmd === cmd);
}

/**
 * Les innstillingene fra basen.
 *
 * Går gjennom `window.api` som alt annet, så fikstursømmen (`e2e/harness.ts`
 * seeder en falsk sqlite-rad) virker her uten at noe i appen vet om den.
 */
export async function hydrateSettings(): Promise<void> {
  try {
    const loaded = await window.api.getSettings();
    settings.value = loaded;
    // Shimmen svarte kanskje med defaults ETTER en feilet lesning — den
    // kaster ikke, den logger og faller tilbake. Feilringen er det eneste
    // stedet forskjellen finnes.
    hydrateError.value = ipcFailedFor("settings_get")
      ? "settingsLoadFailed"
      : null;
  } catch (err) {
    console.warn("[settings] hydrate failed", err);
    settings.value = { ...SETTINGS_DEFAULTS };
    hydrateError.value = "settingsLoadFailed";
  } finally {
    hydrated.value = true;
  }
}

/**
 * Skriv inn en delmengde. Erstatter objektet i stedet for å mutere det —
 * signalet varsler på identitet, og en mutasjon ville vært usynlig.
 */
export function patchSettings(patch: Partial<Settings>): void {
  settings.value = { ...settings.value, ...patch };
}

// ── Lagring ─────────────────────────────────────────────────────────────────
//
// Beslutningene bor i `settings-save-core.ts` og er tabelltestet; dette er
// timeren og IPC-en rundt dem. Én delt promise per byge, akkurat som legacy:
// den som ba om lagringen får vite om skrivningen faktisk landet, og «Lagret ✓»
// kan aldri vises for noe som ikke ble lagret.

let timer: SaveTimerState = IDLE_SAVE_TIMER;
let handle: ReturnType<typeof setTimeout> | null = null;
let pending: Promise<boolean> | null = null;
let settle: ((ok: boolean) => void) | null = null;

async function write(): Promise<boolean> {
  try {
    return !!(await window.api.saveSettings(payloadFor(settings.value)));
  } catch (err) {
    // Et avvist `settings_save` reiser hele veien (R3-B). `false` her er det
    // `useSetting` reverterer på — verdien i UI skal ikke bli stående som om
    // den ble lagret.
    console.warn("[settings] save failed", err);
    return false;
  }
}

function resolvePending(ok: boolean): void {
  const done = settle;
  settle = null;
  pending = null;
  done?.(ok);
}

/**
 * Lagre, etterslepende. Returnerer en promise som løses når skrivningen
 * FAKTISK landet — flere kall i samme byge deler den ene promisen og det ene
 * svaret.
 */
export function saveSettingsDebounced(
  delayMs: number = SAVE_COALESCE_MS,
): Promise<boolean> {
  if (!pending) {
    pending = new Promise<boolean>((resolve) => {
      settle = resolve;
    });
  }
  const plan = planSave(timer, Date.now(), delayMs);
  timer = plan.next;
  if (handle) clearTimeout(handle);
  handle = setTimeout(() => {
    handle = null;
    timer = IDLE_SAVE_TIMER;
    void write().then(resolvePending);
  }, delayMs);
  return pending;
}

/** Skriv nå hvis noe venter. Før navigasjon, før avslutning. */
export async function flushSavePending(): Promise<void> {
  const plan = planFlush(timer);
  timer = plan.next;
  if (plan.action === "none") return;
  if (handle) clearTimeout(handle);
  handle = null;
  resolvePending(await write());
}

/**
 * Glem all ventende lagring uten å skrive. Finnes for tester og for
 * teardown — ALDRI for en vanlig navigasjon, der `flushSavePending` er svaret.
 */
export function cancelSavePending(): void {
  if (handle) clearTimeout(handle);
  handle = null;
  timer = IDLE_SAVE_TIMER;
  resolvePending(false);
}
