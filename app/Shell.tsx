/**
 * Skallets rot — minimalt med vilje.
 *
 * S1a bygger fundamentet: i18n, tilstand, ruteren og `useSetting`. Det VISUELLE
 * er S1b sitt (`PageShell`, komponentbiblioteket, de ekte skjermene). Denne
 * komponenten finnes for at fundamentet skal være observerbart — fra
 * enhetsgaten og fra nettleser-nivået — uten å foregripe én eneste
 * designbeslutning.
 *
 * Overskriften er en KATALOGNØKKEL, slått opp dynamisk på sidenavnet. Det er
 * samtidig beviset for at `tDyn` virker mot et ekte subtre, og for at
 * `@lib/*` når fram til de sju katalogene i stedet for en kopi.
 */

import { route } from "./router/router";
import { t, tDyn } from "./i18n";
import { hydrateError } from "./state/settings";
import { SettingProbe } from "./dev/setting-probe";

export interface ShellProps {
  /** `?probe=<navn>`. TODO(S1b): forsvinner med `app/dev/`. */
  probe?: string | null;
}

export function Shell({ probe }: ShellProps) {
  const current = route.value;
  const failed = hydrateError.value;

  return (
    <main>
      <h1 data-testid="app-heading">{tDyn("app.page", current.page)}</h1>
      {current.tab ? <p data-testid="app-tab">{current.tab}</p> : null}
      {current.anchor ? <p data-testid="app-anchor">{current.anchor}</p> : null}
      {current.firstRun ? (
        <p data-testid="app-first-run">{t("nav.settings")}</p>
      ) : null}
      {/*
        Aldri stille defaults: når `settings_get` feilet svarer api-shimmen med
        SETTINGS_DEFAULTS, og en ødelagt base ser da nøyaktig ut som en
        fabrikkny app. TODO(S1b): dette blir et ordentlig banner.
      */}
      {failed ? (
        <p data-testid="hydrate-error" role="alert">
          {tDyn("error", failed)}
        </p>
      ) : null}
      {probe === "setting" ? <SettingProbe /> : null}
    </main>
  );
}
