/**
 * Bannerne på TA OPP (canvas sett 7) — flyttet ut av `RecordPage.tsx` i E3.
 * Innholdet er uendret: fire stripper, én form, i rekkefølge etter hvor mye
 * det haster.
 */

import type { PreflightFinding } from "@legacy/bindings/PreflightFinding";
import { capitalizeFirst } from "@lib/ui/capitalize";
import { DOT } from "@lib/ui/dot";
import { intlParts, parseLocalIso } from "@lib/status/next-recording-core";

import { locale, t, tDyn, tf, tn } from "../../i18n";
import { navigate } from "../../router/router";
import { interpolate, WARNING_SUFFIXES } from "../../state/backend-warning";
import {
  banners,
  dismissBanner,
  type BackendWarningBanner,
  type BannerData,
} from "../../state/banners";
import { currentRoomMinutes } from "../../state/disk";
import {
  dismissMissed,
  dismissPreflight,
  nextRecording,
} from "../../state/next-recording";
import { LOW_DISK_MINUTES } from "../../state/status-line";
import { Banner } from "../../ui/Banner/Banner";
import { Button } from "../../ui/Button/Button";
import { alertDialog } from "../../ui/dialog";
import { preflightText } from "./preflight-text";
import {
  nativeErrorDetail,
  nativeErrorSuffix,
  qualityReasonSuffix,
  spanOfMinutes,
} from "./record-core";
import styles from "./record.module.css";
import { spanText } from "./span-text";

// ── Bannerne (canvas sett 7) ────────────────────────────────────────────────

/**
 * Fire stripper, én form.
 *
 * Rekkefølgen er hvor mye det haster: noe som ER tapt først, så det som
 * stopper søndagen, så det som stoppet forrige søndag.
 */
export function RecordBanners() {
  // BARE opptakssidens egne. Køen er delt, og P3 la til `update`, som ikke
  // hører til noen side og derfor rendres av skallet — over hvilken side som
  // enn står. Et filter og ikke en else-gren: en nøkkel som havnet her uten å
  // være ventet ville blitt malt som et kvalitetsbanner uten at noe sa fra.
  const list = banners.value.filter(
    (entry) =>
      entry.key === "recording-error" || entry.key === "recording-quality",
  );
  // Bakendens egne advarsler (`backend://warning`) — se
  // `state/backend-warning.ts`, som eier både kanalen og dedupliseringen mot
  // stripene under. De rendres HER og ikke i skallet fordi alle fire handler om
  // opptaket: forhåndsbufferen, gjenopprettingen, lydenheten, disken.
  const warnings = banners.value.filter(isBackendWarning);
  const state = nextRecording.value;
  const room = currentRoomMinutes();
  const lowDisk = room !== null && room < LOW_DISK_MINUTES;

  return (
    <div class={styles.banners}>
      {list.map((entry) =>
        entry.key === "recording-error" ? (
          <Banner
            key={entry.key}
            tone="bad"
            testId="banner-recording-error"
            title={tf("app.banner.errorTitle", {
              time: clockOf(entry.atMs),
            })}
            detail={nativeErrorDetail(
              tDyn("recording", nativeErrorSuffix(entry.code)),
              entry.code,
              entry.message,
            )}
            onDismiss={() => dismissBanner("recording-error")}
            actions={
              <Button
                variant="secondary"
                testId="banner-recording-error-open"
                onClick={() => navigate("recordings")}
              >
                {t("app.banner.errorShow")}
              </Button>
            }
          />
        ) : (
          <Banner
            key={entry.key}
            tone="bad"
            testId="banner-recording-quality"
            title={tf("recording.qualityAlarm", {
              m: entry.measuredSec,
              e: entry.expectedSec,
            })}
            detail={qualityReasonText(entry.reasonCodes, entry.reasons)}
            onDismiss={() => dismissBanner("recording-quality")}
            actions={
              <Button
                variant="secondary"
                testId="banner-recording-quality-open"
                onClick={() => navigate("recordings")}
              >
                {t("recording.qualityAction")}
              </Button>
            }
          />
        ),
      )}

      {warnings.map((entry) => (
        <Banner
          key={entry.key}
          // Bakendens egen alvorlighetsgrad, ikke vår gjetning. `error` er
          // `role="alert"`: en mikser som ikke er i huset en halvtime før et
          // planlagt opptak SKAL avbryte det skjermleseren holder på med.
          tone={entry.severity === "error" ? "bad" : "warn"}
          testId={`banner-${entry.key}`}
          title={warningText(entry.code, entry.msg, entry.params)}
          onDismiss={() => dismissBanner(entry.key)}
        />
      ))}

      {lowDisk ? (
        <Banner
          tone="warn"
          testId="banner-low-disk"
          title={tf("app.banner.diskTitle", {
            room: spanText(spanOfMinutes(room)),
          })}
          detail={t("app.banner.diskDesc")}
          actions={
            <Button
              variant="secondary"
              testId="banner-low-disk-free"
              onClick={() =>
                navigate("record", { panel: "source-more", anchor: "folder" })
              }
            >
              {t("app.banner.diskFree")}
            </Button>
          }
        />
      ) : null}

      {state.missed.length > 0 ? (
        <Banner
          tone="warn"
          testId="banner-missed"
          title={tf("app.banner.missedTitle", {
            when: capitalizeFirst(
              intlParts(locale.value)(parseLocalIso(state.missed[0].at))
                .dateLong,
              locale.value,
            ),
          })}
          detail={
            state.missed.length > 1
              ? tn("missed.banner", state.missed.length)
              : t("app.banner.missedDesc")
          }
          onDismiss={dismissMissed}
          actions={
            <Button
              variant="secondary"
              testId="banner-missed-why"
              onClick={() =>
                void alertDialog({
                  title: t("app.banner.missedWhy"),
                  message: t("app.banner.missedHelp"),
                })
              }
            >
              {t("app.banner.missedWhy")}
            </Button>
          }
        />
      ) : null}

      {state.preflight.length > 0 ? (
        <Banner
          tone="warn"
          testId="banner-preflight"
          title={preflightHeadline(state.preflight)}
          // Funnene i BRUKERENS språk: motoren sender en `PreflightCode`, og
          // `preflightText` slår den opp under `status.preflightCode`. Ingen
          // oppsummering — hvert funn er nøyaktig det funnet motoren gjorde,
          // bare sagt på riktig språk.
          detail={state.preflight.map(preflightText).join(DOT)}
          onDismiss={dismissPreflight}
        />
      ) : null}
    </div>
  );
}

/** «{n} feil må rettes før opptaket» / «{n} ting å se på». Katalognøklene er
 *  legacy-skallets egne og finnes i begge språk — ingen NY tellende nøkkel. */
function preflightHeadline(findings: readonly PreflightFinding[]): string {
  const errors = findings.filter((f) => f.severity === "error").length;
  return errors > 0
    ? tn("status.preflightErrors", errors)
    : tn("status.preflightWarns", findings.length);
}

/**
 * Er dette en bakende-advarsel?
 *
 * En typevakt og ikke et `startsWith` i filteret: `BannerData` er en union, og
 * et filter som ikke SIER at det smalner den lar `entry.severity` være et
 * felt TypeScript ikke vet finnes. Vakten er det ene stedet «hvilke nøkler er
 * bakendens» står skrevet, og `BackendWarningKey` i `state/banners.ts` er
 * listen den holdes mot.
 */
function isBackendWarning(entry: BannerData): entry is BackendWarningBanner {
  return entry.key.startsWith("backend-");
}

/**
 * Årsakslinja under kvalitetsalarmen.
 *
 * ## ⚠️ Den var motorens hardkodede NORSKE prosa
 *
 * `sundayrec_core::selftest` setter sammen setninger som «3.42s manglende/
 * stille lyd — hakking/dropp» med `format!`, og de gikk rett inn i banneret.
 * En engelsk bruker fikk altså norsk teknisk sjargong i det ene varselet som
 * betyr «ikke stol på dette opptaket».
 *
 * Motoren sender nå kodene ved siden av prosaen. Regelen er én linje:
 *
 *   • `reasonCodes === null` — FELTET mangler, altså en eldre bakende. Da er
 *     prosaen alt som finnes, og en sann setning på feil språk slår en tom
 *     linje. (Samme avveining som `backend://warning`s ukjente koder.)
 *   • ellers oversettes hver kode; en kode katalogen ikke kjenner faller
 *     tilbake på motorens prosalinje på SAMME indeks, ikke på en generisk
 *     «ukjent årsak» — den ville byttet informasjon mot språk.
 */
function qualityReasonText(
  codes: readonly string[] | null,
  reasons: readonly string[],
): string | undefined {
  if (codes === null) {
    return reasons.length
      ? tf("recording.qualityReasons", { r: reasons.join(", ") })
      : undefined;
  }
  if (codes.length === 0) return undefined;
  const parts = codes.map((code, i) => {
    const suffix = qualityReasonSuffix(code);
    return suffix ? tDyn("recording", suffix) : (reasons[i] ?? code);
  });
  return tf("recording.qualityReasons", { r: parts.join(", ") });
}

/**
 * Setningen for én bakende-advarsel.
 *
 * Rekkefølgen er legacys, og den er hele designet:
 *
 *   1. `code` → en `notify.*`-nøkkel → setningen på brukerens språk,
 *   2. ellers `msg`, motorens egen (norske) ordlyd,
 *   3. ellers den bare koden.
 *
 * Steg 2 er det som gjør det trygt for bakenden å lære en ny advarsel før
 * denne katalogen gjør det: brukeren får en SANN setning på feil språk i
 * stedet for stillhet. Stillhet er nøyaktig det denne kanalen produserte i
 * månedsvis. `tDyn` kaster i DEV på en ukjent nøkkel, så oppslaget gjøres bare
 * for koder tabellen faktisk kjenner — en ny kode skal falle til steg 2, ikke
 * ta ned siden.
 */
function warningText(
  code: string,
  msg: string | null,
  params: Readonly<Record<string, string>>,
): string {
  const suffix = WARNING_SUFFIXES[code];
  const template = suffix ? tDyn("notify", suffix) : (msg ?? code);
  return interpolate(template, { ...params });
}

/** «11:42» — klokkeslettet i en feilmelding er halve informasjonen. */
function clockOf(atMs: number): string {
  return new Intl.DateTimeFormat(locale.value, {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(atMs));
}
