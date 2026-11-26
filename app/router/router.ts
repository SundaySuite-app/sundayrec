/**
 * Ruteren — tre flater («Én flate»): TA OPP, OPPTAK og INNSTILLINGER (tannhjulet).
 *
 *   `record`     TA OPP — den store knappen; lydkilde-arket, ukestripa og
 *                kameraet bor som PANELER på denne flaten (`route.panel`).
 *   `recordings` OPPTAK — lista er landingsvisningen; et opptak åpner editoren
 *                på SAMME side (pil tilbake), eksporten er et ark inne i
 *                editoren. Papirkurven er en fane (`tab: "trash"`).
 *   `settings`   INNSTILLINGER — det som ikke hører hjemme på de to andre.
 *
 * ## E2: eldre lenker lever videre, men lander på nye steder
 *
 * `plan` er ikke lenger en side. `?goto=plan[:anker]` og `navigate("plan")`
 * løses til `record` med `panel: "strip"` (ukestripa åpen). Innstillingsankrene
 * som flyttet til lydkilde-arket — `sound`, `engine`, `folder`, `autodelete` —
 * løses til `record` med `panel: "source"` (`"source-more"` for de tre siste,
 * som bor under «Mer»). Andre innstillingsankre blir på `settings`.
 * Ankeret følger med i ruten (SettingsPage/PlanPage ruller dit mens de
 * midlertidig står i panelets sted, se `Shell.tsx`); E4/E5 fjerner det.
 *
 * Den frittstående eksportsiden og de eldste `?goto=`-aliasene er borte. Det
 * som er igjen er ÉN regel for ukjente sider: de lander på TA OPP, og sier fra
 * i konsollen i stedet for å vise en tom flate.
 *
 * `?goto=<side>:<fane>` leses fortsatt (e2e og skjermbildepassene bruker den):
 * en fane skrevet som `settings-sound` eller bare `sound` betyr det samme, og
 * på INNSTILLINGER og den gamle PLAN-ruten er «fanen» et anker.
 */

import { computed, signal } from "@preact/signals";
import {
  createTrayDispatcher,
  initTrayActions,
  type TrayActionHandlers,
  type TrayActionId,
} from "@lib/tray-actions";

/** De tre flatene. */
export type Page = "record" | "recordings" | "settings";

/**
 * Panelet som skal stå åpent over TA OPP. `strip` = ukestripa åpen,
 * `source` = lydkilde-arket, `source-more` = arket med «Mer» utfoldet.
 * E3–E5 bygger panelene; E2 setter bare tilstanden.
 */
export type Panel = "strip" | "source" | "source-more";

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
  /** Panelet som skal åpnes på TA OPP (`record`). */
  panel?: Panel;
}

/** Ruten som gjelder nå. */
export const route = signal<Route>({ page: "record" });

/** Panelet ruten ber om — for flatene som skal åpne det. */
export const panel = computed<Panel | undefined>(() => route.value.panel);

/** Sidene, som et sett, så en ukjent id kan kjennes igjen som ukjent. */
const PAGES: readonly Page[] = ["record", "recordings", "settings"];

/** Den gamle PLAN-siden: kjent, men løses til TA OPP + stripa. */
const LEGACY_PLAN = "plan";

/** Innstillingsankre som flyttet til lydkilde-arket, og hvilket panel de åpner. */
const SOURCE_ANCHORS: Readonly<Record<string, Panel>> = {
  sound: "source",
  engine: "source-more",
  folder: "source-more",
  autodelete: "source-more",
};

/** Samme form som legacy `navigateTo`s opsjoner, så et kallsted kan flyttes
 *  fra det ene skallet til det andre uten å skrives om. */
export interface NavigateOpts {
  tab?: string;
  anchor?: string;
  highlight?: boolean;
  firstRun?: boolean;
  panel?: Panel;
}

/** Regn ut ruten uten å sette den — ren, og derfor tabelltestbar. */
export function resolveRoute(page: string, opts: NavigateOpts = {}): Route {
  const legacyPlan = page === LEGACY_PLAN;
  const known = PAGES.includes(page as Page) ? (page as Page) : undefined;

  if (!known && !legacyPlan) {
    // Høylytt, ikke stille: en tom side er den ene tilbakemeldingen som ikke
    // sier noe. Advarsel og ikke feil — en dårlig dyplenke skal ikke telle som
    // en krasj i e2e-nivået.
    console.warn(`[router] ukjent side «${page}» — viser TA OPP`);
  }
  // `plan` er sin egen «side» bare til og med navnet: ankeret tolkes som før.
  const requested: string = legacyPlan ? LEGACY_PLAN : (known ?? "record");

  // `?goto=settings:settings-sound` og `?goto=settings:sound` er det samme.
  const bare = opts.tab?.startsWith(`${requested}-`)
    ? opts.tab.slice(requested.length + 1)
    : opts.tab;
  const asAnchor = legacyPlan || known === "settings";
  const tab = asAnchor ? undefined : bare;
  const anchor = opts.anchor ?? (asAnchor ? bare : undefined);

  let target: Page = legacyPlan ? "record" : (known ?? "record");
  let panelOut: Panel | undefined = opts.panel;
  if (legacyPlan) panelOut = "strip";
  else if (target === "settings" && anchor && anchor in SOURCE_ANCHORS) {
    target = "record";
    panelOut = SOURCE_ANCHORS[anchor];
  }
  if (target !== "record") panelOut = undefined;

  const next: Route = { page: target };
  if (opts.firstRun) next.firstRun = true;
  if (tab) next.tab = tab;
  if (panelOut) next.panel = panelOut;
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
 *  LYDEN, og hørselstesten bor i lydkilde-arket på TA OPP (E2). */
const ACTION_PAGE: Record<TrayActionId, Page> = {
  "start-recording": "record",
  "stop-recording": "record",
  "open-recordings-folder": "recordings",
  "run-preflight": "record",
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
    navigate(ACTION_PAGE[action], { panel: "source", anchor: "sound" });
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
