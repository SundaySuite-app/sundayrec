/**
 * Oppdatering — status, kanal og «automatisk».
 *
 * Tilstandene bor i `app/state/update-core.ts`, som en TOTAL funksjon fra fase
 * til visning. Grunnen står der: legacy malte den samme raden fra sju
 * uavhengige lyttere som hver skrev tre steder, og feilen som fantes i to
 * utgivelser var en knapp fra én fase som ble stående inn i en annen —
 * «Start på nytt og installer» under «Du er oppdatert».
 *
 * ## Statuslinja ER «se etter nå»
 *
 * Linja under ordet er en knapp: trykk = sjekk nå (sperret mens det sjekkes
 * eller lastes ned, med grunnen). Den manuelle sjekken er med vilje ugatet av
 * «Automatisk» — PRIVACY.md har den som sitt ene unntak, fordi et trykk er
 * eieren som spør.
 *
 * ## Installer-knappen finnes bare når det er noe å installere
 *
 * `view.action` er `null` ellers. En knapp fra én fase kan ikke overleve inn i
 * en annen.
 *
 * ## Hvorfor raden ikke abonnerer selv
 *
 * ÉN lytter, i `app/state/auto-update.ts`; raden leser signalet. Fasen
 * overlever dermed også at tannhjulet lukkes: en nedlasting som fortsetter
 * mens man ser på OPPTAK er en nedlasting raden kan gjøre rede for når man
 * åpner den igjen.
 *
 * ## «Automatisk»
 *
 * `autoUpdate` har en bakendleser likevel: renderen eier den timesvise
 * sjekken, og PRIVACY.md lover at den kan slås av: «Slår du den av, tar appen
 * ikke kontakt med serveren — verken ved oppstart eller den vanlige sjekken
 * hver time.» Se `app/state/auto-update.ts`.
 */

import { t, tf } from "../../../i18n";
import { useSetting } from "../../../settings/use-setting";
import { updatePhase } from "../../../state/auto-update";
import { isRecording } from "../../../state/recording";
import { updateView, type UpdateView } from "../../../state/update-core";
import { Button } from "../../../ui/Button/Button";
import { Receipt } from "../../../ui/Receipt/Receipt";
import { Segmented } from "../../../ui/Segmented/Segmented";
import { Toggle } from "../../../ui/Toggle/Toggle";
import { GearRow } from "./GearRow";
import styles from "./gear.module.css";
import notes from "./UpdateRow.module.css";

export function UpdateRow() {
  const view = updateView(updatePhase.value);

  const channel = useSetting("updateChannel", {
    kind: "select",
    // Bare mot BETA. Å gå tilbake til den trygge kanalen er gratis — en vakt på
    // veien ut ville vært friksjon på det ansvarlige valget.
    confirmIf: (value) =>
      String(value) === "beta"
        ? {
            title: t("app.setup.advanced.updateBetaTitle"),
            message: t("app.setup.advanced.updateBetaBody"),
            confirmLabel: t("app.setup.advanced.updateBetaConfirm"),
            cancelLabel: t("app.setup.advanced.updateBetaCancel"),
          }
        : null,
  });
  const auto = useSetting("autoUpdate", { kind: "toggle" });

  const message = view.message;
  const failed =
    message?.key === "updateFailed" || message?.key === "updateRestartFailed";
  const text = message ? messageText(message) : t("app.gear.updateIdle");

  return (
    <GearRow
      testId="gear-row-update"
      anchors={["update", "updates", "autoupdate"]}
      icon="refresh"
      title={t("app.gear.update")}
      sub={
        <>
          <button
            type="button"
            data-testid="adv-update-check"
            data-tone={view.tone ?? "neutral"}
            aria-disabled={view.canCheck ? undefined : "true"}
            title={
              view.canCheck
                ? t("app.gear.updateCheck")
                : t("app.setup.advanced.updateChecking")
            }
            class={styles.status}
            onClick={() => {
              if (view.canCheck) void window.api.checkForUpdates();
            }}
          >
            {/* Feil har sin egen id og `role="alert"`: en feilet sjekk er
                beskjeden, og den skal leses opp selv om fokus står andre steder. */}
            <span
              data-testid={failed ? "adv-update-error" : "adv-update-state"}
              role={failed ? "alert" : undefined}
            >
              {text}
            </span>
          </button>
          <Receipt state={channel.receipt} testId="gear-update-receipt" />
        </>
      }
      controls={
        <>
          <Segmented
            testId="gear-channel"
            label={t("app.gear.channel")}
            value={String(channel.draft ?? "stable")}
            disabled={channel.busy}
            options={[
              {
                value: "stable",
                label: t("app.setup.advanced.updateStable"),
                testId: "gear-channel-stable",
              },
              {
                value: "beta",
                label: t("app.setup.advanced.updateBeta"),
                testId: "gear-channel-beta",
              },
            ]}
            onChange={(next) => channel.set(next)}
          />
          <Toggle
            checked={auto.draft === true}
            onChange={(next) => auto.set(next)}
            disabled={auto.busy}
            labelId="gear-auto-update-label"
            testId="adv-auto-update-control-input"
          />
          <span id="gear-auto-update-label" hidden>
            {t("app.gear.autoUpdate")}
          </span>
        </>
      }
      extra={
        view.action || view.notes ? (
          <>
            {view.action ? (
              <Button
                variant="primary"
                busy={view.action.busy}
                // Både «Last ned og installer» og «Start på nytt og installer»
                // ender i en prosess som byttes ut — og en nedlasting kjeder
                // rett videre til omstarten. Motoren avviser kallet uansett;
                // dette er så den frivillige får VITE hvorfor, i stedet for
                // en feil.
                disabled={isRecording.value}
                disabledReason={t("app.setup.advanced.updateBusyRecording")}
                testId="adv-update-install"
                onClick={() => void window.api.installUpdate()}
              >
                {view.action.key === "install"
                  ? t("app.setup.advanced.updateRestart")
                  : t("app.setup.advanced.updateDownload")}
              </Button>
            ) : null}
            {view.notes ? (
              <div class={notes.notes} data-testid="adv-update-notes">
                <div class={notes.notesLabel}>
                  {t("app.setup.advanced.updateNotesLabel")}
                </div>
                <div class={notes.notesText}>{view.notes}</div>
              </div>
            ) : null}
          </>
        ) : undefined
      }
    />
  );
}

/** Nøkkel + innsettinger → setning. Kjernen velger ALDRI tekst. */
function messageText(message: NonNullable<UpdateView["message"]>): string {
  switch (message.key) {
    case "updateChecking":
      return t("app.setup.advanced.updateChecking");
    case "updateUpToDate":
      return t("app.setup.advanced.updateUpToDate");
    case "updateAvailable":
      return tf("app.setup.advanced.updateAvailable", { v: message.version });
    case "updateDownloading":
      return tf("app.setup.advanced.updateDownloading", {
        pct: message.percent,
      });
    case "updateReady":
      return tf("app.setup.advanced.updateReady", { v: message.version });
    case "updateRestarting":
      return t("app.setup.advanced.updateRestarting");
    case "updateRestartFailed":
      return t("app.setup.advanced.updateRestartFailed");
    case "updateFailed":
      return t("app.setup.advanced.updateFailed");
  }
}
