/**
 * Ruteren — fire sider: TA OPP, OPPTAK, PLAN og INNSTILLINGER.
 *
 * ## F5: fire sider, ikke fire jobber i DaVinci-rekkefølge
 *
 * Rent bord beskar appen til kjernen: ta opp, finn opptaket igjen og klipp det,
 * eksporter, og la en ukeplan starte opptak av seg selv. Sidene følger det:
 *
 *   `record`     TA OPP — den store knappen, kilden og nivået, kameraet og linja
 *                «neste planlagte opptak».
 *   `recordings` OPPTAK — lista er landingsvisningen; et opptak åpner editoren
 *                på SAMME side (pil tilbake), eksporten er et ark inne i
 *                editoren. Papirkurven er en fane (`tab: "trash"`).
 *   `plan`       PLAN — den ukentlige tidsplanen. F6 bytter flaten mot en
 *                grafisk ukevisning; slot-logikken (`slots-core`,
 *                `schedule-core`) er skilt fra visningen.
 *   `settings`   INNSTILLINGER — lyd og kanaler, kamera, lagringsmappe, varsler,
 *                språk, oppdatering, personvern, om og logg.
 *
 * Den frittstående eksportsiden og `?goto=`-aliasene fra de eldre sidene er
 * borte. Det som er igjen er ÉN regel: en ukjent side lander på TA OPP, og sier
 * fra i konsollen i stedet for å vise en tom flate.
 *
 * `?goto=<side>:<fane>` leses fortsatt (e2e og skjermbildepassene bruker den):
 * en fane skrevet som `settings-sound` eller bare `sound` betyr det samme, og
 * på INNSTILLINGER og PLAN er «fanen» et anker som siden ruller til.
 */

import { signal } from "@preact/signals";
import {
  createTrayDispatcher,
  initTrayActions,
  type TrayActionHandlers,
  type TrayActionId,
} from "@lib/tray-actions";

/** De fire sidene. */
export type Page = "record" | "recordings" | "plan" | "settings";

export interface Route {
  page: Page;
  /** Fane inne i siden, i det NYE navnerommet. */
  tab?: string;
  /** Elementet man kom for — en bar id, ikke en selektor. S1b ruller dit. */
  anchor?: string;
  /** Puls på ankeret. Standard `true` når det er et anker — å komme et sted
   *  uten å skjønne hvorfor er feilmodusen dette erstatter. */
  highlight?: boolean;
  /** Første gang appen åpnes: OPPSETT er ikke et sted brukeren valgte å gå,
   *  det er starten. S1b bruker flagget til å vise en annen ramme rundt siden
   *  — og ruten «husker» ikke flagget, for det gjelder bare denne ankomsten. */
  firstRun?: boolean;
}

/** Ruten som gjelder nå. */
export const route = signal<Route>({ page: "record" });

/** Sidene, som et sett, så en ukjent id kan kjennes igjen som ukjent. */
const PAGES: readonly Page[] = ["record", "recordings", "plan", "settings"];

/** Sider der en `tab` fra `?goto=` er et rullemål (et anker), ikke en fane. */
const ANCHOR_PAGES: readonly Page[] = ["plan", "settings"];

/** Samme form som legacy `navigateTo`s opsjoner, så et kallsted kan flyttes
 *  fra det ene skallet til det andre uten å skrives om. */
export interface NavigateOpts {
  tab?: string;
  anchor?: string;
  highlight?: boolean;
  firstRun?: boolean;
}

/** Regn ut ruten uten å sette den — ren, og derfor tabelltestbar. */
export function resolveRoute(page: string, opts: NavigateOpts = {}): Route {
  const known = PAGES.includes(page as Page) ? (page as Page) : undefined;

  if (!known) {
    // Høylytt, ikke stille: en tom side er den ene tilbakemeldingen som ikke
    // sier noe. Advarsel og ikke feil — en dårlig dyplenke skal ikke telle som
    // en krasj i e2e-nivået.
    console.warn(`[router] ukjent side «${page}» — viser TA OPP`);
  }
  const target: Page = known ?? "record";

  // `?goto=settings:settings-sound` og `?goto=settings:sound` er det samme.
  const bare = opts.tab?.startsWith(`${target}-`)
    ? opts.tab.slice(target.length + 1)
    : opts.tab;
  const asAnchor = known !== undefined && ANCHOR_PAGES.includes(known);
  const tab = asAnchor ? undefined : bare;
  const anchor = opts.anchor ?? (asAnchor ? bare : undefined);

  const next: Route = { page: target };
  if (opts.firstRun) next.firstRun = true;
  if (tab) next.tab = tab;
  if (anchor) {
    next.anchor = anchor;
    next.highlight = opts.highlight !== false;
  }
  return next;
}

/** Gå et sted. Samme signatur som legacy `navigateTo`. */
export function navigate(page: string, opts: NavigateOpts = {}): void {
  route.value = resolveRoute(page, opts);
}

/**
 * Legg `window.showPage` på plass.
 *
 * Kontrakten er ikke pynt: `e2e/harness.ts` VENTER på at den skal bli en
 * funksjon før et spec får lov til å gjøre noe, api-shimmens `?goto=`-blokk
 * poller på den, og tray/dyplenker går gjennom den. Skallet er ikke oppe før
 * den finnes.
 *
 * `nav` kan overstyres av verten. Den ENE grunnen står i `app/main.tsx`:
 * shimmens `?goto=`-blokk navigerer på nytt 150 ms etter modullast, og den
 * gjentakelsen skal slippes hvis noe allerede har navigert i mellomtiden.
 */
export function installGlobalNavigation(
  nav: (id: string) => void = (id) => navigate(id),
): void {
  window.showPage = nav;
}

// ── Tray ────────────────────────────────────────────────────────────────────

/**
 * Handlingen menylinje-ikonet ba om, og som ingen har utført ennå.
 *
 * Et SIGNAL, ikke et `getElementById(...).click()`. Legacy-skallets tray-hooks
 * syntetiserer klikk på knapper som må finnes, på en side som må være vist, i
 * en DOM som må være ferdig bygget — tre forutsetninger som alle har sviktet
 * hver for seg. Her navigerer ruteren dit handlingen hører hjemme og lar
 * flaten plukke den opp når den er klar.
 */
export const pendingAction = signal<TrayActionId | null>(null);

/** Hvor hver handling hører hjemme. «Sjekk oppsettet» er et spørsmål om
 *  LYDEN, og hørselstesten bor under Lyd og kanaler i INNSTILLINGER. */
const ACTION_PAGE: Record<TrayActionId, Page> = {
  "start-recording": "record",
  "stop-recording": "record",
  "open-recordings-folder": "recordings",
  "run-preflight": "settings",
};

/** Ta imot handlingen (og tøm den). Kalles av flaten som utfører den. */
export function consumePendingAction(): TrayActionId | null {
  const action = pendingAction.peek();
  if (action) pendingAction.value = null;
  return action;
}

function arm(action: TrayActionId): void {
  // «Sjekk oppsettet» har ingen handling å vente på: å lande på lydkortet er
  // hele svaret.
  if (action === "run-preflight") {
    navigate(ACTION_PAGE[action], { anchor: "sound" });
    return;
  }
  pendingAction.value = action;
  navigate(ACTION_PAGE[action]);
}

/**
 * Håndtererne, ett sted. `stop-recording` er med selv om dirigentens liste
 * nevnte fire: Rust STOPPER motoren selv og sender eventet som et signal til
 * UI-et om å følge etter, så å ikke lytte på den ville latt skjermen påstå at
 * det går et opptak som er slutt.
 */
const trayHandlers: TrayActionHandlers = {
  startRecording: () => arm("start-recording"),
  stopRecording: () => arm("stop-recording"),
  openRecordingsFolder: () => arm("open-recordings-folder"),
  runPreflight: () => arm("run-preflight"),
};

/**
 * Rut én tray-payload. Eksportert (og ren) så tabellen kan testes uten et
 * abonnement. `false` = en id vi ikke kjenner, som skal ignoreres og ikke
 * kaste inne i en event-callback der ingenting fanger den.
 */
export function handleTrayAction(payload: unknown): boolean {
  return createTrayDispatcher(trayHandlers)(payload);
}

/** Abonner på `tray://action`. Legacy-modulens skall gjør jobben; vi bidrar
 *  bare med håndtererne. */
export function installTrayNavigation(): void {
  initTrayActions(trayHandlers);
}
