/**
 * Samtykkekortet og kvitteringen («Opptaket er lagret») — flyttet ut av
 * `RecordPage.tsx` i E3, uendret.
 */

import { useEffect, useState } from "preact/hooks";

import type { TelemetryConsent } from "@legacy/bindings/TelemetryConsent";
import { DOT } from "@lib/ui/dot";

import { openInEditor } from "../../editor/entry";
import { locale, t } from "../../i18n";
import {
  dismissFinishedRecording,
  finishedRecording,
} from "../../state/recording";
import { lastRecording, loadRecordingCount } from "../../state/recordings";
import { settings } from "../../state/settings";
import { Button } from "../../ui/Button/Button";
import { Card } from "../../ui/Card/Card";
import { ConsentCard } from "../../ui/ConsentCard/ConsentCard";
import { reveal, revealLabel } from "../../ui/reveal";
import { showTelemetryPreview } from "../settings/advanced/TelemetryRow";
import { basename, formatBytes, spanOfSeconds } from "./record-core";
import styles from "./record.module.css";
import { spanText } from "./span-text";

// ── Samtykkekortet ──────────────────────────────────────────────────────────

/**
 * Spurt ÉN gang, på OPPTAK (canvas sett 6 flyttet det ut av sekvensen).
 *
 * `needsPrompt` er bakendens svar, ikke vårt: den er sann når ingen har svart
 * ennå, OG igjen den dagen omfanget utvides — også for den som sa nei sist.
 */
export function Consent() {
  const [consent, setConsent] = useState<TelemetryConsent | null>(null);

  useEffect(() => {
    void window.api
      .telemetryConsentGet()
      .then(setConsent)
      // En probe vi ikke fikk kjørt er ikke en grunn til å spørre — et kort som
      // dukker opp fordi IPC-en glapp er et spørsmål brukeren ikke kan svare på.
      .catch(() => setConsent(null));
  }, []);

  if (consent?.needsPrompt !== true) return null;
  return (
    <ConsentCard
      status={consent.status}
      onExplain={() => void showTelemetryPreview()}
      onAnswered={() => setConsent({ ...consent, needsPrompt: false })}
    />
  );
}

// ── Kvitteringen (2.6) ──────────────────────────────────────────────────────

/**
 * «Opptaket er lagret» — et KORT, ikke en toast.
 *
 * Erstatter legacy `#editor-prompt-toast`, som spurte «vil du redigere?» og
 * forsvant av seg selv. Kortet blir stående til noen har sett det: at fila
 * finnes, hvor stor den ble, og hvor den ligger.
 *
 * Varigheten og størrelsen leses fra historikkraden bakenden skriver FØR den
 * fyrer `recording://finished` (`finalize_one` i `recorder/engine.rs`) — ikke
 * fra overleggets egen klokke. Klokken vet hvor lenge det har gått; bare fila
 * vet hva som faktisk ble skrevet, og det er forskjellen datatapsalarmen
 * finnes for.
 *
 * «Åpne i Rediger» er med fra P4a: flaten finnes, og den er det man som oftest
 * vil gjøre med et opptak som nettopp ble ferdig. Den er PRIMÆR og «Vis i
 * Finder» sekundær — Finder er der man går når man skal gjøre noe utenfor
 * appen, og redigering er inne i den.
 *
 * ⚠️ `askOpenEditor` har fortsatt ingen leser i Rust (ATLAS §2.6), så kortet
 * vises uansett hva den innstillingen sier.
 *
 * ## F1-R4: kortet henter seg selv fram
 *
 * Kvitteringen rendres NEDERST i `.page` — under kilde, måler, Start, de to
 * opptakskortene og hele høyrekolonnen. Med et utfoldet kontrollrom er den
 * langt under det synlige vinduet, og en frivillig som ikke ruller ned aner
 * ikke at fila ble lagret. Samme historie for en tastaturbruker: ingenting
 * flyttet fokus, så skjermleseren sa aldri at noe nytt kom.
 *
 * Så når `finished` går fra `null` til et opptak, ruller siden kortet til
 * midten og gir det fokus. `tabIndex` settes IMPERATIVT på DOM-noden og ikke som en prop
 * på `Card` (som ikke videresender den): kortet skal være fokuserbart bare i
 * det øyeblikket det dukker opp, ikke stå i tabrekkefølgen for alltid.
 * `preventScroll` på selve `focus()`-kallet fordi rullingen allerede er gjort
 * med `block: "center"` — nettleserens egen fokus-scroll retter seg bare inn
 * mot nærmeste kant, og ville flyttet kortet vekk fra midten igjen.
 */
/**
 * Raden kvitteringens «Rediger» og «Vis i Finder» handler på — den EKTE veien
 * inn i begge, som navngir en rad og aldri en sti (A2, B-familien). Rust skrev
 * raden rett før `recording://finished` og la id-en i hendelsen; at den ikke
 * er der betyr at raden ikke ble skrevet, og da finnes ingenting å handle på.
 * Som reserve (en hendelse uten id) kan historikken ha lest raden inn mens
 * kvitteringen sto: samme fil, samme rad. Aldri en søk på stien etterpå.
 */
function finishedRowId(
  finished: { path: string; recordingId: string | null },
  loaded: { path?: string; id?: string | null } | null | undefined,
): string | null {
  if (finished.recordingId) return finished.recordingId;
  return loaded?.path === finished.path ? (loaded.id ?? null) : null;
}

export function Done() {
  const finished = finishedRecording.value;
  const rows = lastRecording.value;

  // Historikken leses på nytt når et opptak er ferdig — det nye opptaket ER
  // den nyeste raden, og «Siste opptak»-kortet skal ikke vise det forrige.
  useEffect(() => {
    if (finished) void loadRecordingCount();
  }, [finished?.path]);

  // F1-R4: rull til og fokuser kvitteringen når den DUKKER OPP — ikke på hver
  // gjengivelse mens den samme `finished` står (historikkraden som kommer inn
  // et øyeblikk senere skal ikke rive fokus bort igjen).
  useEffect(() => {
    if (!finished) return;
    const el = document.querySelector<HTMLElement>(
      '[data-testid="record-done"]',
    );
    if (!el) return;
    el.tabIndex = -1;
    el.scrollIntoView({ block: "center" });
    el.focus({ preventScroll: true });
  }, [finished?.path]);

  if (!finished) return null;

  const row = rows?.path === finished.path ? rows : null;
  const rowId = finishedRowId(finished, rows);
  // Se `LastRecordingCard`: 0 er «ukjent», ikke «null sekunder».
  const span = spanOfSeconds(row?.durationSec || null);
  const size = formatBytes(row?.fileSizeBytes ?? null, locale.value);
  const folder = (settings.value.saveFolder ?? "").trim();
  const meta = [span.kind === "none" ? "" : spanText(span), size, folder]
    .filter(Boolean)
    .join(DOT);

  return (
    <Card
      tone="good"
      testId="record-done"
      title={t("app.done.title")}
      description={meta || undefined}
    >
      <div data-testid="record-done-file" class={styles.value}>
        {row?.filename ?? basename(finished.path)}
      </div>
      <div class={styles.row}>
        {/*
          Uten rad finnes ingenting å åpne eller vise, og knappene sier det
          (som i biblioteket) i stedet for å lukke seg uten et ord.
        */}
        <Button
          variant="primary"
          disabled={!rowId}
          disabledReason={t("app.done.revealFailed")}
          testId="record-done-edit"
          onClick={() =>
            openInEditor(
              rowId ?? undefined,
              row?.startedAt ?? row?.timestamp ?? null,
              basename(finished.path),
            )
          }
        >
          {t("editor.promptOpen")}
        </Button>
        <Button
          variant="secondary"
          disabled={!rowId}
          disabledReason={t("app.done.revealFailed")}
          testId="record-done-reveal"
          onClick={() => void reveal(rowId)}
        >
          {revealLabel()}
        </Button>
        <Button
          variant="ghost"
          testId="record-done-ok"
          onClick={dismissFinishedRecording}
        >
          {t("app.done.ok")}
        </Button>
      </div>
    </Card>
  );
}
