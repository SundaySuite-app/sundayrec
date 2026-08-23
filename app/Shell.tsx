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
 *   OPPSETT   de fem spørsmålene med SVARET som står nå. «Ikke satt opp» der
 *             det ikke er svart. Ingen «Endre»-knapp, fordi skjermene den
 *             skulle åpne ikke finnes ennå.
 *
 * ## Overlays er søsken av `#app`
 *
 * `DialogHost` og `ToastHost` rendres i et EGET Preact-tre, inn i
 * `#overlays`. Grunnen står i DialogHost: verten setter `inert` på `#app`
 * mens en dialog er åpen, og en dialog inne i `#app` ville slått av seg selv.
 */

import { t, tDyn } from "./i18n";
import { navigate, route } from "./router/router";
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
import { VuMeter } from "./ui/VuMeter/VuMeter";

export interface ShellProps {
  /** `?probe=<navn>`. TODO(P): forsvinner med `app/dev/`. */
  probe?: string | null;
}

export function Shell({ probe }: ShellProps) {
  const current = route.value;
  const failed = hydrateError.value;

  return (
    <PageShell>
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
      ) : current.page === "record" ? (
        <RecordPlaceholder />
      ) : current.page === "library" ? (
        <LibraryPlaceholder />
      ) : (
        <SetupPlaceholder />
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

// ── OPPSETT ─────────────────────────────────────────────────────────────────

/**
 * De fem spørsmålene, med svaret som gjelder nå.
 *
 * `null` som svar betyr «ikke satt opp», og raden blir gul. Det er ikke pynt:
 * spørsmål 5 (hvem får beskjed) er den vanligste tomme innstillingen i
 * eierens egen profil, og en gul rad er hele grunnen til at man oppdager det
 * før en søndag i stedet for etterpå.
 */
function SetupPlaceholder() {
  const s = settings.value;
  const format = (s.format ?? "mp3").toLowerCase();
  const quality =
    format === "flac" || format === "wav" || format === "mp3" ? format : null;

  const rows: Array<{ id: string; question: string; answer: string | null }> = [
    {
      id: "sound",
      question: t("app.setup.q1"),
      answer: s.deviceName ?? s.deviceId,
    },
    { id: "folder", question: t("app.setup.q2"), answer: s.saveFolder },
    {
      id: "quality",
      question: t("app.setup.q3"),
      answer: quality ? tDyn("app.setup.quality", quality) : null,
    },
    { id: "church", question: t("app.setup.q4"), answer: s.churchName || null },
    {
      id: "alerts",
      question: t("app.setup.q5"),
      // Adressen teller bare når varslingen faktisk er PÅ. En lagret adresse
      // med bryteren av betyr at ingen får beskjed.
      answer: s.emailOnError && s.emailAddress ? s.emailAddress : null,
    },
  ];

  return (
    <>
      <p data-testid="setup-lede">{t("app.setup.lede")}</p>
      {rows.map((row) => (
        <Card
          key={row.id}
          testId={`setup-row-${row.id}`}
          tone={row.answer ? "neutral" : "warn"}
          title={row.question}
          description={
            row.answer ??
            (row.id === "alerts"
              ? t("app.setup.nobodyYet")
              : t("app.setup.notSetUp"))
          }
        />
      ))}
    </>
  );
}
