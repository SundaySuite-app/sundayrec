/**
 * Skallets rot — skinnen, og det de tre destinasjonene faktisk kan si i dag.
 *
 * ## Hva som er ekte her, og hva som ikke er det
 *
 * Skinnen, statuslinjen, navigasjonen og fokusflyttingen er FERDIG: det er
 * S1b sitt. Sidene er ikke bygget ennå — de er fase P — så hver destinasjon
 * viser den delen av seg selv som allerede er sann.
 *
 * Og BARE den delen. Ingenting her sier «kommer senere», og ingen knapp
 * finnes uten at den gjør noe. En død knapp lærer en frivillig at knappene i
 * denne appen ikke er til å stole på, og den lærdommen overlever lenge etter
 * at knappen er koblet.
 *
 * Derfor leser hver plassholder ekte tilstand i stedet for å påstå noe:
 *
 *   OPPTAK    ER bygget (P2): kilde, hørsel, Start, opptaksoverlegget,
 *             stopp-bekreftelsen og kvitteringen. Se `app/pages/record/`.
 *   BIBLIOTEK antall opptak telles på ekte (`recordings_list`). Null →
 *             tomtilstanden. Flere → hvor de ligger. Aldri «ingen opptak» på
 *             en maskin som har tolv.
 *   OPPSETT   ER bygget (P1a + P1b): de fem spørsmålene med svaret som står
 *             nå, de fem skjermene «Endre» åpner, de to tilleggene og
 *             Avansert. Se `app/pages/setup/`.
 *
 * ## Første gang, og samtykkekortet
 *
 * `route.firstRun` bytter ut HELE innholdet med sekvensen (`FirstRun`): de
 * samme fem skjermene, ett spørsmål om gangen. Skinnen står, fordi den er
 * stedet appen er.
 *
 * Samtykkekortet hører til OPPTAK og ikke til sekvensen — canvasens sett 6
 * flyttet det ut med vilje. Det bor nå i `RecordPage`, der det hører hjemme.
 *
 * ## Overlays er søsken av `#app`
 *
 * `DialogHost` og `ToastHost` rendres i et EGET Preact-tre, inn i
 * `#overlays`. Grunnen står i DialogHost: verten setter `inert` på `#app`
 * mens en dialog er åpen, og en dialog inne i `#app` ville slått av seg selv.
 *
 * ## Menylinjens «Åpne opptaksmappen»
 *
 * Den ene tray-handlingen som ikke hører til noen side: den åpner en mappe i
 * Finder og er ferdig. Den plukkes derfor opp her, i skallet, som alltid er
 * montert — en side som må være vist før handlingen kan skje er nøyaktig
 * antakelsen legacy-hookene brøt på.
 */

import { useEffect } from "preact/hooks";

import { t, tDyn } from "./i18n";
import { RecordPage } from "./pages/record/RecordPage";
import { FirstRun, firstRunHeading } from "./pages/setup/FirstRun";
import { SetupPage, setupHeading } from "./pages/setup/SetupPage";
import {
  consumePendingAction,
  navigate,
  pendingAction,
  route,
} from "./router/router";
import { SettingProbe } from "./dev/setting-probe";
import { recordingCount } from "./state/recordings";
import { hydrateError, settings } from "./state/settings";
import { Banner } from "./ui/Banner/Banner";
import { Button } from "./ui/Button/Button";
import { Card } from "./ui/Card/Card";
import { Chip } from "./ui/Chip/Chip";
import { DialogHost } from "./ui/DialogHost/DialogHost";
import { EmptyState } from "./ui/EmptyState/EmptyState";
import { PageShell } from "./ui/PageShell/PageShell";
import { ToastHost } from "./ui/ToastHost/ToastHost";

export interface ShellProps {
  /** `?probe=<navn>`. TODO(P): forsvinner med `app/dev/`. */
  probe?: string | null;
}

export function Shell({ probe }: ShellProps) {
  const current = route.value;
  const failed = hydrateError.value;
  useTrayFolder();

  const firstRun = current.firstRun === true;

  return (
    <PageShell heading={firstRunHeading(firstRun) ?? setupHeading(current.tab)}>
      {/*
        Aldri stille defaults: når `settings_get` feilet svarer api-shimmen med
        SETTINGS_DEFAULTS, og en ødelagt base ser da nøyaktig ut som en
        fabrikkny app. Feilringen er det eneste stedet forskjellen finnes.
      */}
      {failed ? (
        <Banner
          tone="bad"
          title={tDyn("error", failed)}
          testId="hydrate-error"
        />
      ) : null}

      {probe === "setting" ? (
        <SettingProbe />
      ) : firstRun ? (
        <FirstRun />
      ) : current.page === "record" ? (
        <RecordPage />
      ) : current.page === "library" ? (
        <LibraryPlaceholder />
      ) : (
        <SetupPage />
      )}
    </PageShell>
  );
}

/** Dialog- og toastverten. Montert i `#overlays` — se toppen av fila. */
export function Overlays() {
  return (
    <>
      <DialogHost />
      <ToastHost />
    </>
  );
}

/**
 * Menylinjens «Åpne opptaksmappen».
 *
 * Handlingen trenger ingen side — den åpner en mappe og er ferdig — så den
 * plukkes opp i skallet, som alltid er montert. Ruteren har allerede navigert
 * til BIBLIOTEK, slik at man også SER opptakene man nettopp ba om å få se.
 *
 * Bare denne ene id-en tas imot her; de andre blir stående til flaten sin, og
 * derfor `peek` på signalet før det tømmes.
 */
function useTrayFolder(): void {
  const armed = pendingAction.value;
  useEffect(() => {
    if (armed !== "open-recordings-folder") return;
    consumePendingAction();
    const folder = (settings.peek().saveFolder ?? "").trim();
    if (folder) void window.api.openFolder(folder);
  }, [armed]);
}

// ── BIBLIOTEK ───────────────────────────────────────────────────────────────

function LibraryPlaceholder() {
  const count = recordingCount.value;

  // Ikke lest ennå: ingen påstand i noen retning.
  if (count === null) return null;

  if (count === 0) {
    return (
      <EmptyState
        testId="library-empty"
        title={t("app.library.empty")}
        description={t("app.library.emptyDesc")}
        action={
          <Button
            variant="primary"
            testId="library-go-record"
            onClick={() => navigate("record")}
          >
            {t("app.library.goRecord")}
          </Button>
        }
      />
    );
  }

  return (
    <Card
      testId="library-stored"
      title={t("app.library.stored")}
      description={settings.value.saveFolder ?? undefined}
      actions={<Chip tone="neutral">{count}</Chip>}
    />
  );
}
