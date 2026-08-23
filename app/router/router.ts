/**
 * Ruteren — tre sider, og én tabell som oversetter alt det gamle til dem.
 *
 * ## Fire jobber, tre sider
 *
 * Legacy-skallet har fem sider (`home`, `schedule`, `search`, `settings`,
 * `editor`) og fem faner inne i innstillinger. «Frivilligen først» folder det
 * til tre: TA OPP, OPPTAKENE og OPPSETT. En frivillig som aldri har sett appen
 * skal ikke måtte vite hvilken av fem sider «filnavn» bor på.
 *
 * ## Hvorfor aliastabellene ikke bare er teknisk gjeld
 *
 * `?goto=settings:audio` står i et titalls e2e-spec, i skjermbilde-passene, i
 * tray-menyen og i lenker vi ikke har funnet. Legacy har allerede en slik
 * tabell (`ui/navigate.ts`), og kommentaren over den sier hvorfor den skal
 * BEHOLDES: den er billigere enn å jakte hvert kallsted hver gang
 * informasjonsarkitekturen flytter seg, og en dyplenke som stille åpner feil
 * fane er verre enn en som feiler høylytt.
 *
 * Så: hver gamle id fortsetter å lande riktig, og en ukjent id sier fra i
 * konsollen i stedet for å vise en tom side.
 */

import { signal } from "@preact/signals";
import {
  createTrayDispatcher,
  initTrayActions,
  type TrayActionHandlers,
  type TrayActionId,
} from "@lib/tray-actions";

/** De tre sidene. */
export type Page = "record" | "library" | "setup";

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
const PAGES: readonly Page[] = ["record", "library", "setup"];

/**
 * Gamle SIDE-id-er → nye sider.
 *
 * `search` er Historikk (det er ingen `history`-side — se e2e/history.spec.ts),
 * og den hører hjemme sammen med opptakene. `schedule` og `settings` er begge
 * oppsett nå.
 */
export const PAGE_ALIASES: Record<string, Page> = {
  home: "record",
  search: "library",
  editor: "library",
  settings: "setup",
  schedule: "setup",
};

/** Hvor en gammel FANE (eller en gammel side med en naturlig fane) lander. */
export interface TabTarget {
  page: Page;
  tab?: string;
  anchor?: string;
}

/**
 * Gamle fane-id-er → ny side + fane.
 *
 * Nøklene er de fullt kvalifiserte id-ene `parseGoto` produserer
 * (`settings:audio` → `settings-audio`), pluss de to SIDENE som har en
 * selvsagt fane i den nye arkitekturen (`editor`, `schedule`) — de kommer inn
 * uten fane og ville ellers landet på siden sin standardfane og mistet
 * poenget med lenken.
 *
 * De to nederste er PENSJONERTE id-er fra før legacy foldet sju faner til fem;
 * `legacy/renderer/ui/navigate.ts` sender dem videre til `settings-sharing`, og
 * her går de samme vei videre.
 */
export const TAB_ALIASES: Record<string, TabTarget> = {
  "settings-audio": { page: "setup", tab: "sound" },
  "settings-video": { page: "setup", tab: "addons", anchor: "camera" },
  "settings-files": { page: "setup", tab: "files" },
  "settings-sharing": { page: "setup", tab: "advanced", anchor: "sharing" },
  "settings-general": { page: "setup", tab: "advanced" },
  "settings-publish": { page: "setup", tab: "advanced", anchor: "sharing" },
  "settings-notifications": {
    page: "setup",
    tab: "advanced",
    anchor: "sharing",
  },
  editor: { page: "library", tab: "edit" },
  schedule: { page: "setup", tab: "schedule" },
};

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
  const byTab = opts.tab ? TAB_ALIASES[opts.tab] : undefined;
  const byPage = TAB_ALIASES[page];
  const aliased = PAGE_ALIASES[page];
  const known = PAGES.includes(page as Page) ? (page as Page) : undefined;
  const target = byTab?.page ?? byPage?.page ?? aliased ?? known;

  if (!target) {
    // Høylytt, ikke stille: en tom side er den ene tilbakemeldingen som ikke
    // sier noe. Advarsel og ikke feil — en dårlig dyplenke skal ikke telle som
    // en krasj i e2e-nivået.
    console.warn(`[router] ukjent side «${page}» — viser TA OPP`);
  }

  // En fane som ikke er en gammel id sendes videre som den er: det er allerede
  // et navn i det nye navnerommet.
  const tab =
    byTab?.tab ?? (opts.tab && !byTab ? opts.tab : undefined) ?? byPage?.tab;
  const anchor = opts.anchor ?? byTab?.anchor ?? byPage?.anchor;

  const next: Route = { page: target ?? "record" };
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
 */
export function installGlobalNavigation(): void {
  window.showPage = (id: string) => navigate(id);
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

/** Hvor hver handling hører hjemme. S1b kobler selve utførelsen. */
const ACTION_PAGE: Record<TrayActionId, Page> = {
  "start-recording": "record",
  "stop-recording": "record",
  "open-recordings-folder": "library",
  "run-preflight": "record",
  "run-diagnostics": "setup",
};

/** Ta imot handlingen (og tøm den). Kalles av flaten som utfører den. */
export function consumePendingAction(): TrayActionId | null {
  const action = pendingAction.peek();
  if (action) pendingAction.value = null;
  return action;
}

function arm(action: TrayActionId): void {
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
  runDiagnostics: () => arm("run-diagnostics"),
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
