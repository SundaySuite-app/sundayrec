/**
 * Skallets rot — skinnen, og det destinasjonene faktisk kan si i dag.
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
 *   OPPTAK      ER bygget (P2): kilde, hørsel, Start, opptaksoverlegget,
 *               stopp-bekreftelsen og kvitteringen. Se `app/pages/record/`.
 *   REDIGERING  ER bygget (P3 + P4a): lista, søket, slett-med-angre og
 *               papirkurven (`app/pages/library/`), og arbeidsflaten når en fil
 *               er åpen (`app/editor/`).
 *   EKSPORTERING ER bygget (P4b, egen destinasjon fra D3): valgene, kjøringen
 *               og kvitteringen — og, uten en åpen fil, det sist redigerte
 *               opptaket med én knapp. Se `app/pages/export/`.
 *   OPPSETT     ER bygget (P1a + P1b): de fem spørsmålene med svaret som står
 *               nå, de fem skjermene «Endre» åpner, de to tilleggene og
 *               Avansert. Se `app/pages/setup/`.
 *
 * ## REDIGERING har to visninger, og BRYTEREN er `loadState`
 *
 * Papirkurven er en fane (`route.tab`); ellers avgjør lastetilstanden: `idle`
 * ⇒ biblioteket, alt annet ⇒ arbeidsflaten. `loadState` og ikke `hasFile`,
 * fordi `openFile` setter den SYNKRONT før første `await` — `filePath` fylles
 * også synkront, men lastingen har tre tilstander biblioteket ikke kan vise
 * («laster», «kunne ikke åpnes»), og en bryter som bare visste om det var en
 * sti ville sendt en feilet åpning tilbake til lista uten å si hvorfor.
 *
 * Grenen er derfor også der bibliotek-blaffet ville bodd hvis den ble snudd:
 * `e2e/editor.spec.ts` har en spec som ser DOM-en under klikket og krever at
 * lista aldri står der mens fila åpnes.
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
 * `DialogHost`, `ToastHost` og `RecordingOverlay` rendres i et EGET
 * Preact-tre, inn i `#overlays`. Grunnen står i DialogHost: verten setter
 * `inert` på `#app` mens en dialog er åpen, og en dialog inne i `#app` ville
 * slått av seg selv. Opptaksoverlegget er der av to grunner til: det skal
 * ligge OVER skinnen, og det skal ikke rives ned av et rutebytte — et opptak
 * som går er ikke en side man er på.
 *
 * ## Oppdateringsbanneret hører til skallet
 *
 * En ny versjon er ikke en side man er på, så stripa står over den siden man
 * ER på — samme sted som `hydrate-error`, rett under overskriften. Tilstanden
 * bor i den delte bannerkøen (`state/banners.ts`) og fylles av
 * `state/auto-update.ts`; her er bare flaten. Aldri en egen toast: canvasens
 * sett 7 har ÉN toast-form, og «det finnes en oppdatering» er ikke en
 * kvittering som skal forsvinne av seg selv.
 *
 * ## Menylinjens «Åpne opptaksmappen»
 *
 * Den ene tray-handlingen som ikke hører til noen side: den åpner en mappe i
 * Finder og er ferdig. Den plukkes derfor opp her, i skallet, som alltid er
 * montert — en side som må være vist før handlingen kan skje er nøyaktig
 * antakelsen legacy-hookene brøt på.
 */

import { useEffect } from "preact/hooks";

import { EditorPage, editorHeading } from "./editor/EditorPage";
import { loadState } from "./editor/model";
import { locale, t, tDyn, tf } from "./i18n";
import {
  DropZone,
  libraryHeading,
  LibraryPage,
  TRASH_TAB,
} from "./pages/library/LibraryPage";
import { TrashPage } from "./pages/library/TrashPage";
import { ExportPage } from "./pages/export/ExportPage";
import { RecordPage } from "./pages/record/RecordPage";
import { RecordingOverlay } from "./pages/record/RecordingOverlay";
import { FirstRun, firstRunHeading } from "./pages/setup/FirstRun";
import { SetupPage } from "./pages/setup/SetupPage";
import { consumePendingAction, pendingAction, route } from "./router/router";
import { SettingProbe } from "./dev/setting-probe";
import { banners, dismissBanner } from "./state/banners";
import { hydrateError, settings } from "./state/settings";
import { Banner } from "./ui/Banner/Banner";
import { Button } from "./ui/Button/Button";
import { DialogHost } from "./ui/DialogHost/DialogHost";
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
    /*
      OPPTAK, EKSPORTERING og INNSTILLINGER heter det destinasjonen heter — alle
      er ÉN skjerm, og et eget navn på en av dem ville vært en overskrift som
      lover en underside. REDIGERING har fortsatt to andre ting å hete
      (Papirkurv, og datoen på opptaket som er åpent), og første gang er fem
      spørsmål med hvert sitt.
    */
    <PageShell
      heading={
        firstRunHeading(firstRun) ??
        (current.page === "edit" ? editHeading(current.tab) : undefined)
      }
    >
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

      <UpdateBanner />

      {probe === "setting" ? (
        <SettingProbe />
      ) : firstRun ? (
        <FirstRun />
      ) : current.page === "record" ? (
        <RecordPage />
      ) : current.page === "edit" ? (
        <EditSurface tab={current.tab} />
      ) : current.page === "export" ? (
        <ExportPage />
      ) : (
        <SetupPage />
      )}
    </PageShell>
  );
}

/**
 * REDIGERING: papirkurven, arbeidsflaten eller biblioteket — i den rekkefølgen.
 *
 * Slippsonen ligger rundt de to SISTE. Papirkurven er utenfor med vilje: å
 * slippe et opptak på papirkurven ville sett ut som en handling, og den ene
 * handlingen det ligner på er den vi ikke gjør.
 */
function EditSurface({ tab }: { tab: string | undefined }) {
  if (tab === TRASH_TAB) return <TrashPage />;
  return (
    <DropZone>
      {loadState.value === "idle" ? <LibraryPage /> : <EditorPage />}
    </DropZone>
  );
}

/** Overskriften REDIGERING skal ha: papirkurvens, opptakets dato, eller
 *  destinasjonens eget navn (utelatt ⇒ «Redigering» fra katalogen). */
function editHeading(tab: string | undefined): string | undefined {
  if (tab === TRASH_TAB) return libraryHeading(tab);
  return loadState.value === "idle" ? undefined : editorHeading();
}

/** Dialog-, toast- og opptaksverten. Montert i `#overlays` — se toppen av fila. */
export function Overlays() {
  return (
    <>
      <RecordingOverlay />
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
 * til REDIGERING, slik at man også SER opptakene man nettopp ba om å få se.
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

// ── Oppdateringsbanneret ────────────────────────────────────────────────────

/**
 * «Versjon X er klar» — over den siden som står, uansett hvilken.
 *
 * Tre tilstander, og de bærer de SAMME katalognøklene som raden under
 * Avansert. Ikke kopierte setninger: kopier driver fra hverandre, og to steder
 * som sier hver sin ting om den samme nedlastingen er nøyaktig skjøten dette
 * skallet er skrevet for å unngå. Fasen bak dem er også den samme — én lytter,
 * i `state/auto-update.ts`.
 *
 * `warn` og ikke `bad`: en oppdatering som venter er ikke noe som er galt.
 * `bad` er `role="alert"` og avbryter en skjermleser midt i noe annet, og det
 * har den ikke fortjent.
 */
function UpdateBanner() {
  const entry = banners.value.find((b) => b.key === "update");
  if (entry?.key !== "update") return null;

  const install = (
    <Button
      variant="secondary"
      testId="banner-update-install"
      onClick={() => void window.api.installUpdate()}
    >
      {entry.state === "ready"
        ? t("app.setup.advanced.updateRestart")
        : t("app.setup.advanced.updateDownload")}
    </Button>
  );

  return (
    <Banner
      tone="warn"
      testId="banner-update"
      title={
        entry.state === "available"
          ? tf("app.setup.advanced.updateAvailable", { v: entry.version })
          : entry.state === "downloading"
            ? tf("app.setup.advanced.updateDownloading", {
                pct: entry.percent.toLocaleString(locale.value),
              })
            : tf("app.setup.advanced.updateReady", { v: entry.version })
      }
      // Ingen knapp mens den laster ned: det er ingenting å be om, og en
      // knapp som bare kan trykkes forgjeves er en knapp som lærer folk at
      // knappene her ikke betyr noe.
      actions={entry.state === "downloading" ? undefined : install}
      onDismiss={() => dismissBanner("update")}
    />
  );
}
