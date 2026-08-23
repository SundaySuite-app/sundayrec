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
 *   OPPTAK    ingen lydkilde valgt → kortet som sier det, og knappen til
 *             OPPSETT. Er kilden valgt, står den ekte VU-måleren der og
 *             svarer på «hører vi lyd?».
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
 * flyttet det ut med vilje. Det står her, over plassholderen, så P4 arver det
 * når den ekte opptakssiden bygges: kortet er ferdig, plassen er kjent.
 *
 * ## Overlays er søsken av `#app`
 *
 * `DialogHost` og `ToastHost` rendres i et EGET Preact-tre, inn i
 * `#overlays`. Grunnen står i DialogHost: verten setter `inert` på `#app`
 * mens en dialog er åpen, og en dialog inne i `#app` ville slått av seg selv.
 */

import { useEffect, useState } from "preact/hooks";

import { t, tDyn } from "./i18n";
import { FirstRun, firstRunHeading } from "./pages/setup/FirstRun";
import { showTelemetryPreview } from "./pages/setup/advanced/TelemetryRow";
import { SetupPage, setupHeading } from "./pages/setup/SetupPage";
import { navigate, route } from "./router/router";
import { SettingProbe } from "./dev/setting-probe";
import { recordingCount } from "./state/recordings";
import { hydrateError, settings } from "./state/settings";
import { Banner } from "./ui/Banner/Banner";
import { Button } from "./ui/Button/Button";
import { Card } from "./ui/Card/Card";
import { Chip } from "./ui/Chip/Chip";
import { ConsentCard } from "./ui/ConsentCard/ConsentCard";
import { DialogHost } from "./ui/DialogHost/DialogHost";
import { EmptyState } from "./ui/EmptyState/EmptyState";
import { PageShell } from "./ui/PageShell/PageShell";
import { ToastHost } from "./ui/ToastHost/ToastHost";
import { VuMeter } from "./ui/VuMeter/VuMeter";

export interface ShellProps {
  /** `?probe=<navn>`. TODO(P): forsvinner med `app/dev/`. */
  probe?: string | null;
}

export function Shell({ probe }: ShellProps) {
  const current = route.value;
  const failed = hydrateError.value;

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
        <RecordPlaceholder />
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

// ── OPPTAK ──────────────────────────────────────────────────────────────────

function RecordPlaceholder() {
  const s = settings.value;
  const source = s.deviceName ?? s.deviceId;

  return (
    <>
      <Consent />
      <Source source={source} />
    </>
  );
}

/**
 * Samtykkekortet, spurt ÉN gang.
 *
 * `needsPrompt` er bakendens svar, ikke vårt: den er sann når ingen har svart
 * ennå, OG igjen den dagen omfanget utvides — også for den som sa nei sist.
 * Kortet forsvinner først når svaret FAKTISK er lagret (se `ConsentCard`).
 */
function Consent() {
  const [ask, setAsk] = useState(false);

  useEffect(() => {
    void window.api
      .telemetryConsentGet()
      .then((consent) => setAsk(consent?.needsPrompt === true))
      // En probe vi ikke fikk kjørt er ikke en grunn til å spørre — et kort som
      // dukker opp fordi IPC-en glapp er et spørsmål brukeren ikke kan svare på.
      .catch(() => setAsk(false));
  }, []);

  if (!ask) return null;
  return (
    <ConsentCard
      onExplain={() => void showTelemetryPreview()}
      onAnswered={() => setAsk(false)}
    />
  );
}

function Source({ source }: { source: string | null }) {
  const s = settings.value;

  if (!source) {
    return (
      <Card
        tone="warn"
        testId="record-no-source"
        title={t("app.record.noSource")}
        description={t("app.record.noSourceDesc")}
        actions={
          <Button
            variant="primary"
            testId="record-choose-sound"
            onClick={() => navigate("setup")}
          >
            {t("app.record.chooseSound")}
          </Button>
        }
      />
    );
  }

  // Kilden er valgt: da er «hører vi den?» det eneste spørsmålet som betyr
  // noe, og måleren svarer på det på ekte.
  return (
    <Card testId="record-listening" description={source}>
      <VuMeter deviceName={s.deviceName} testId="record-vu" />
    </Card>
  );
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
