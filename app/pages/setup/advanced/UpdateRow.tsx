/**
 * «Oppdateringer» — kanalen, sjekken, og den ene knappen som gjør noe.
 *
 * Tilstandene bor i `update-core.ts`, som en TOTAL funksjon fra fase til
 * visning. Grunnen står der: legacy maler den samme raden fra sju uavhengige
 * lyttere som hver skriver tre steder, og feilen som fantes i to utgivelser var
 * en knapp fra én fase som ble stående inn i en annen — «Start på nytt og
 * installer» under «Du er oppdatert».
 *
 * ## Hvorfor hendelser og ikke polling
 *
 * Tauris oppdaterer-kommandoer polles, men det er api-shimmen som poller dem:
 * den eier nedlastingsløkka OG dødmannsbryteren for den gjentakende «omstarten
 * skjedde ikke»-feilen, og sender ut sju Electron-formede hendelser underveis.
 * `e2e/auto-update.spec.ts` hviler på nøyaktig de sju. En andre poller her ville
 * vært to maskiner som er nesten enige om samme løp.
 *
 * `window.api.on(...)` er derfor riktig sted — og en abonnering er ikke en
 * kommando, så den er ikke noe rekkeviddegaten har en mening om (samme regel som
 * `scheduler://`-eventene i `app/state/next-recording.ts`).
 */

import { useEffect, useState } from "preact/hooks";

import { t, tf } from "../../../i18n";
import { useSetting } from "../../../settings/use-setting";
import { Button } from "../../../ui/Button/Button";
import { Chip } from "../../../ui/Chip/Chip";
import { Select } from "../../../ui/Select/Select";
import { SettingRow } from "../../../ui/SettingRow/SettingRow";
import {
  phaseFromEvent,
  UPDATE_CHANNELS,
  updateView,
  type UpdatePhase,
  type UpdateView,
} from "./update-core";

export function UpdateRow() {
  const [phase, setPhase] = useState<UpdatePhase>({ kind: "idle" });
  const view = updateView(phase);

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

  useEffect(() => {
    const offs = UPDATE_CHANNELS.map((name) =>
      window.api.on(name, (payload: unknown) => {
        const next = phaseFromEvent(name, payload);
        if (next) setPhase(next);
      }),
    );
    return () => {
      for (const off of offs) off?.();
    };
  }, []);

  const message = view.message;
  const failed =
    message?.key === "updateFailed" || message?.key === "updateRestartFailed";

  return (
    <SettingRow
      label={t("app.setup.advanced.update")}
      description={t("app.setup.advanced.updateDesc")}
      receipt={channel.receipt}
      // En feilet oppdatering er ikke en valideringsfeil på kanal-velgeren, men
      // feillinja er der teksten hører hjemme: rett under kontrollen, i rødt,
      // med `role="alert"`. Ingen egen feilflate for én rad.
      error={failed && message ? messageText(message) : null}
      testId="adv-update"
    >
      {(ids) => (
        <>
          {message && !failed ? (
            <Chip tone={chipTone(view)} testId="adv-update-state">
              {messageText(message)}
            </Chip>
          ) : null}
          {view.action ? (
            <Button
              variant="primary"
              busy={view.action.busy}
              testId="adv-update-install"
              onClick={() => void window.api.installUpdate()}
            >
              {view.action.key === "install"
                ? t("app.setup.advanced.updateRestart")
                : t("app.setup.advanced.updateDownload")}
            </Button>
          ) : null}
          <Button
            variant="ghost"
            disabled={!view.canCheck}
            disabledReason={t("app.setup.advanced.updateChecking")}
            testId="adv-update-check"
            onClick={() => void window.api.checkForUpdates()}
          >
            {t("app.setup.advanced.updateCheck")}
          </Button>
          <Select
            value={String(channel.draft ?? "stable")}
            options={[
              { value: "stable", label: t("app.setup.advanced.updateStable") },
              { value: "beta", label: t("app.setup.advanced.updateBeta") },
            ]}
            onChange={(next) => channel.set(next)}
            disabled={channel.busy}
            labelId={ids.labelId}
            describedBy={ids.describedBy}
            testId="adv-update-channel-control-input"
          />
        </>
      )}
    </SettingRow>
  );
}

/** Kjernens tone → brikkas. `bad` males aldri som brikke (den blir feillinje),
 *  og `null` kommer aldri hit — begge deler holdes av kallstedet over. */
function chipTone(view: UpdateView): "neutral" | "good" | "warn" {
  return view.tone === "good"
    ? "good"
    : view.tone === "warn"
      ? "warn"
      : "neutral";
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
