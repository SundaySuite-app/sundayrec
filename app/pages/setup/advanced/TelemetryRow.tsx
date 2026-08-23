/**
 * «Del anonym diagnostikk» — bryteren, «Vis» og «Slett mine data».
 *
 * ## Sannheten er IKKE i innstillingene
 *
 * Samtykket bor i sin egen tilstandsmaskin i Rust
 * (`telemetry_consent_get`/`_set`), ikke i `Settings`. Så dette er den ene raden
 * på skjermen som ikke går gjennom `useSetting` med en nøkkel — den leser og
 * skriver bakenden direkte, og `active` (ikke `status === "granted"`) er
 * fasiten: et gammelt ja til et smalere omfang er ikke et ja til dagens.
 *
 * ## Ingen bekreftelse på å slå PÅ
 *
 * Legacy har ingen `confirmIf` her, og det er med vilje: å legge friksjon bare
 * på ja-siden av et frivillig valg er et mørkt mønster med motsatt fortegn.
 * «Slett mine data» spør, fordi det er handlingen som ikke kan angres.
 *
 * ## «Vis» viser det EKTE
 *
 * `telemetry_preview_payload` svarer med den faktiske neste nyttelasten, ferdig
 * formatert av Rust. Vi rører ikke JSON-en. Er samtykket av, svarer bakenden med
 * formen bygget av lokal historikk og sier det selv (`isNextPayload: false`) —
 * og teksten sier det videre. Et fabrikkert eksempel her ville vært et løfte om
 * kode som aldri kjører.
 */

import { useEffect, useState } from "preact/hooks";

import type { TelemetryConsent } from "@lib/../bindings/TelemetryConsent";

import { t } from "../../../i18n";
import { Button } from "../../../ui/Button/Button";
import { alertDialog, confirmDialog } from "../../../ui/dialog";
import { SettingRow } from "../../../ui/SettingRow/SettingRow";
import { Toggle } from "../../../ui/Toggle/Toggle";
import { toast } from "../../../ui/toast";
import type { Receipt as ReceiptState } from "../../../settings/use-setting-core";

/** Samtykket slik bakenden ser det. `null` = ikke lest ennå. */
export function useConsent(): {
  consent: TelemetryConsent | null;
  refresh: () => Promise<void>;
} {
  const [consent, setConsent] = useState<TelemetryConsent | null>(null);

  async function refresh(): Promise<void> {
    const next = await window.api.telemetryConsentGet().catch(() => null);
    setConsent(next);
  }

  // Én lesning per montering. Samtykket endres bare herfra og fra
  // samtykkekortet, og begge kaller `refresh` selv etterpå — så det finnes
  // ingen avhengighet å reagere på.
  useEffect(() => {
    void refresh();
  }, []);

  return { consent, refresh };
}

/** Åpne forhåndsvisningen. Delt med samtykkekortets «Hva sendes?». */
export async function showTelemetryPreview(): Promise<void> {
  const preview = await window.api.telemetryPreviewPayload().catch(() => null);
  if (!preview) {
    await alertDialog({
      title: t("app.setup.advanced.diagPreviewTitle"),
      message: t("app.setup.advanced.diagPreviewFailed"),
      tone: "error",
    });
    return;
  }
  const hints = [
    preview.isNextPayload
      ? t("app.setup.advanced.diagPreviewNext")
      : t("app.setup.advanced.diagPreviewHistory"),
  ];
  if (preview.isEmpty) hints.push(t("app.setup.advanced.diagPreviewEmpty"));
  await alertDialog({
    title: t("app.setup.advanced.diagPreviewTitle"),
    message: hints.join(" "),
    preformatted: preview.json,
  });
}

export function TelemetryRow() {
  const { consent, refresh } = useConsent();
  const [receipt, setReceipt] = useState<ReceiptState>("idle");
  const [busy, setBusy] = useState(false);

  async function setGranted(next: boolean): Promise<void> {
    if (busy) return;
    setBusy(true);
    setReceipt("saving");
    try {
      // `null` = IPC-en feilet. Aldri en oppdiktet «lagret» — hele poenget med
      // «spør én gang» er at et tapt svar må spørres om på nytt.
      const result = await window.api.telemetryConsentSet(next);
      setReceipt(result ? "saved" : "failed");
      if (!result) toast("error", t("general.saveFailed"));
      await refresh();
    } finally {
      setBusy(false);
    }
  }

  async function deleteData(): Promise<void> {
    const ok = await confirmDialog({
      title: t("app.setup.advanced.diagDeleteTitle"),
      message: t("app.setup.advanced.diagDeleteBody"),
      confirmLabel: t("app.setup.advanced.diagDeleteConfirm"),
      cancelLabel: t("app.setup.cancel"),
      danger: true,
    });
    if (!ok) return;
    const done = await window.api
      .telemetryRegenerateInstallId()
      .catch(() => false);
    toast(
      done ? "success" : "error",
      done
        ? t("app.setup.advanced.diagDeleted")
        : t("app.setup.advanced.diagDeleteFailed"),
    );
    if (done) await refresh();
  }

  return (
    <SettingRow
      label={t("app.setup.advanced.diag")}
      description={t("app.setup.advanced.diagDesc")}
      receipt={receipt}
      testId="adv-diag"
    >
      {(ids) => (
        <>
          <Button
            variant="ghost"
            testId="adv-diag-preview"
            onClick={() => void showTelemetryPreview()}
          >
            {t("app.setup.advanced.show")}
          </Button>
          <Button
            variant="ghost"
            testId="adv-diag-delete"
            onClick={() => void deleteData()}
          >
            {t("app.setup.advanced.diagDelete")}
          </Button>
          <Toggle
            // `active`, ikke `status === "granted"`: et ja til et smalere
            // omfang er ikke et ja til dagens, og bakenden er det ene stedet
            // som vet forskjellen.
            checked={consent?.active === true}
            onChange={(next) => void setGranted(next)}
            disabled={busy}
            labelId={ids.labelId}
            describedBy={ids.describedBy}
            testId="adv-diag-control-input"
          />
        </>
      )}
    </SettingRow>
  );
}
