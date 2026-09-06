/**
 * 5 — Hvem får beskjed hvis noe går galt? (canvasens artboard 5.3)
 *
 * ## Én bryter for OS-varsler, ikke to
 *
 * Bakenden har `notifyStart` og `notifyStop`, og dagens app har en bryter for
 * hver. Ingen frivillig har et forhold til den forskjellen: enten sier maskinen
 * fra om opptaket, eller så gjør den ikke det. Så: ÉN bryter som skriver begge.
 *
 * ⚠️ Canvasens tekst her sa «Alltid på» ved siden av en bryter som kan slås av.
 * Den motsigelsen er rettet — teksten sier hva bryteren gjør, ikke hva vi
 * skulle ønske den gjorde.
 *
 * ## «Sendes via SundaySuite» er SANT nå
 *
 * Fram til reléet var det en løgn, og fila sa det: det fantes ingen
 * SundaySuite-tjeneste som sendte disse e-postene, bare menighetens egen
 * SMTP-server — som en frivillig ikke har og ikke skal skaffe. Reléet lukket
 * det hullet, og skjermen er skrevet om rundt den nye sannheten:
 *
 *   • Reléet er HOVEDVEIEN. Adressen bekreftes én gang (dobbel opt-in: vi
 *     sender en lenke, brukeren trykker på den), og så går varslene gjennom
 *     `notify.sundaysuite.app`. Ingen server, ingen app-passord.
 *   • SMTP er ALTERNATIVET, og det vinner når det finnes: en menighet som
 *     allerede har satt opp en server er uendret av alt dette, av konstruksjon
 *     (`plan_failure`s utledede regel). SmtpCard sier den ene setningen.
 *
 * Bryteren står derfor ikke lenger bak en gate som krever SMTP, men bak
 * `relayGateStatus`: grønn når ÉN av de to veiene er åpen. Gaten er fortsatt
 * trygg her — den slår ikke av sine egne oppsettsfelter, for SMTP-feltene bor
 * under Avansert, og «Bekreft e-postadressen» står i adressekortet UNDER
 * gaten, ikke i den.
 *
 * ⚠️ Denne fila eier UI-teksten. PRIVACY-kapitlet, APP-SHELL og
 * røyk-runbooken er A6.
 *
 * ## Adressen lagres EKSPLISITT
 *
 * Den ene innstillingen i appen der en halvskrevet verdi er aktivt skadelig:
 * «post@» er en adresse ingenting kommer fram til, og du oppdager det den
 * dagen opptaket faktisk feiler. Så `useDraftForm` med Lagre/Avbryt, og en
 * feilet lagring ruller IKKE tilbake — det er noe brukeren skrev. Reléet
 * skjerper begrunnelsen i stedet for å svekke den: en påmelding bruker en
 * ekte e-post og en av endepunktets tre bekreftelser per døgn, så «Bekreft»
 * går på den LAGREDE adressen og er av så lenge det står noe uslagret i
 * feltet.
 */

import { useEffect, useState } from "preact/hooks";

import { errorCode } from "@lib/error-code-core";
import { emailBlockReason } from "@lib/ui/feature-gate-core";

import { locale, t, tf } from "../../i18n";
import { useDraftForm } from "../../settings/use-draft-form";
import { usePatch } from "../../settings/use-patch";
import { useReceipt } from "../../settings/use-receipt";
import { useSetting } from "../../settings/use-setting";
import { emailFacts, refreshEmailFacts } from "../../state/email";
import { relayFacts, refreshRelayFacts } from "../../state/relay";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
} from "../../state/settings";
import { Button } from "../../ui/Button/Button";
import { Card } from "../../ui/Card/Card";
import { Chip } from "../../ui/Chip/Chip";
import { Gate } from "../../ui/Gate/Gate";
import { SettingRow } from "../../ui/SettingRow/SettingRow";
import { Select } from "../../ui/Select/Select";
import { TextField } from "../../ui/TextField/TextField";
import { Toggle } from "../../ui/Toggle/Toggle";
import { toast } from "../../ui/toast";
import { relayGateStatus } from "./decisions-core";
import {
  relayView,
  resendWaitMinutes,
  type ConfirmBlock,
  type RelayStep,
  type RelayView,
} from "./relay-core";
import { autoRecordOn } from "./schedule-core";
import styles from "./setup.module.css";
import { SubPage } from "./SubPage";

/** Samme regex som legacy bruker på det samme feltet. */
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/** Minuttene «påminnelse før opptak» tilbyr. 0 = av. */
const REMINDER_CHOICES = [0, 5, 10, 15, 30, 60];

/** Hvor ofte nedtellingen på «Send på nytt» males på nytt mens den løper. */
const COOLDOWN_TICK_MS = 1000;

export function NotifyPage() {
  const s = settings.value;
  const facts = emailFacts.value;
  const relay = relayFacts.value;
  const gate = relayGateStatus(facts, relay);

  useEffect(() => {
    void refreshEmailFacts();
    // Reléets status er FERSKVARE på denne skjermen: den endrer seg av at noen
    // trykker på en lenke i en innboks, altså utenfor appen. Uten en lesning
    // ved åpning ville siden stått og tilbudt «Send på nytt» til en adresse
    // som ble bekreftet i går. (`refreshRelayFacts` har generasjonsvernet, så
    // gjentatte kall er trygge.)
    void refreshRelayFacts();
  }, []);

  // ── OS-varsler: én bryter, to nøkler ──────────────────────────────────────
  // `usePatch` og ikke en håndlagd lagring: sekvensen (anvend → skriv →
  // kvittering | rull tilbake) er den samme som `useSetting` kjører, og
  // kvitteringen teller ned i stedet for å bli stående som «Lagret ✓» til
  // siden forlates.
  const osOn = s.notifyStart || s.notifyStop;
  const osNotify = usePatch();

  const emailOn = useSetting("emailOnError", {
    kind: "toggle",
    after: () => void refreshEmailFacts(),
  });

  const receipt = useSetting("emailReceiptEnabled", { kind: "toggle" });

  const reminder = useSetting("reminderMinutes", { kind: "select" });
  // Flagget OG en tid — en påminnelse før et opptak som ikke er armert er en
  // beskjed om noe som ikke skal skje.
  const autoOn = autoRecordOn(s);

  // Kvitteringsbryteren RENDRES bare på et bekreftet abonnement, og det er
  // ikke pyntehensyn: kvitteringen finnes bare på reléveien (`plan_receipt`),
  // så en bryter uten den ville vært en av-og-på for noe som ikke kan skje.
  const confirmed =
    relay?.endpointBuilt === true && relay.state === "confirmed";

  return (
    <SubPage lede={t("app.setup.notify.lede")} testId="setup-notify">
      <Card testId="notify-card">
        <SettingRow
          label={t("app.setup.notify.os")}
          description={t("app.setup.notify.osDesc")}
          receipt={osNotify.receipt}
          testId="notify-os"
        >
          {(ids) => (
            <Toggle
              checked={osOn}
              onChange={(next) =>
                void osNotify.write({ notifyStart: next, notifyStop: next })
              }
              disabled={osNotify.busy}
              labelId={ids.labelId}
              describedBy={ids.describedBy}
              testId="notify-os-control-input"
            />
          )}
        </SettingRow>

        <Gate
          status={gate}
          testId="notify-email-gate"
          chipText={
            gate === "unavailable"
              ? t("app.setup.notify.gateChipUnavailable")
              : t("app.setup.notify.gateChip")
          }
          explanation={
            gate === "unavailable"
              ? t("app.setup.notify.gateNoFeature")
              : t("app.setup.notify.gateSmtp")
          }
        >
          <SettingRow
            label={t("app.setup.notify.mail")}
            description={t("app.setup.notify.mailDesc")}
            receipt={emailOn.receipt}
            error={emailOn.error}
            testId="notify-email"
          >
            {(ids) => (
              <Toggle
                checked={emailOn.draft === true}
                onChange={(next) => emailOn.set(next)}
                disabled={emailOn.busy}
                labelId={ids.labelId}
                describedBy={ids.describedBy}
                testId="notify-email-control-input"
              />
            )}
          </SettingRow>

          {confirmed ? (
            <SettingRow
              label={t("app.setup.notify.receipt")}
              description={t("app.setup.notify.receiptDesc")}
              receipt={receipt.receipt}
              error={receipt.error}
              testId="notify-receipt"
            >
              {(ids) => (
                <Toggle
                  checked={receipt.draft === true}
                  onChange={(next) => receipt.set(next)}
                  disabled={receipt.busy}
                  labelId={ids.labelId}
                  describedBy={ids.describedBy}
                  testId="notify-receipt-control-input"
                />
              )}
            </SettingRow>
          ) : null}
        </Gate>
      </Card>

      <AddressCard />

      <Card testId="notify-reminder-card">
        <Gate
          status={autoOn ? "ok" : "unconfigured"}
          testId="notify-reminder-gate"
          chipText={t("app.setup.notify.remindChip")}
          explanation={t("app.setup.notify.remindGate")}
        >
          <SettingRow
            label={t("app.setup.notify.remind")}
            description={t("app.setup.notify.remindDesc")}
            receipt={reminder.receipt}
            testId="notify-reminder"
          >
            {(ids) => (
              <Select
                value={String(reminder.draft ?? 0)}
                options={REMINDER_CHOICES.map((n) => ({
                  value: String(n),
                  label:
                    n === 0
                      ? t("app.setup.notify.remindOff")
                      : tf("app.setup.notify.minutes", { n }),
                }))}
                onChange={(next) => reminder.set(Number(next))}
                disabled={reminder.busy}
                labelId={ids.labelId}
                describedBy={ids.describedBy}
                testId="notify-reminder-control-input"
              />
            )}
          </SettingRow>
        </Gate>
      </Card>
    </SubPage>
  );
}

/**
 * De tre oppslagene er `switch`-er og ikke tabeller fra tilstand til
 * NØKKEL-streng, og det er ikke smakssak: `check-i18n-keys` krever at hver
 * `t()` får en literal nøkkel (en dynamisk nøkkel er en nøkkel gaten ikke kan
 * telle, og dermed en oversettelse som kan forsvinne uten at noe blir rødt).
 * `tDyn` er den andre sanksjonerte veien; her ville den krevd et eget subtre
 * for tre-fire strenger som ellers gjenbrukes fra nabotekster.
 */
const STEP_TONE = {
  unavailable: "neutral",
  none: "neutral",
  pending: "warn",
  confirmed: "good",
  suppressed: "bad",
} as const;

/** Brikketeksten for hver tilstand. */
function stepChip(step: RelayStep): string {
  switch (step) {
    case "unavailable":
      return t("app.setup.notify.gateChipUnavailable");
    case "none":
      return t("app.setup.notify.relayNone");
    case "pending":
      return t("app.setup.notify.relayPending");
    case "confirmed":
      return t("app.setup.notify.relayConfirmed");
    case "suppressed":
      return t("app.setup.notify.relaySuppressed");
  }
}

/** Hvorfor «Bekreft e-postadressen» er av, i klartekst. */
function confirmBlockReason(block: ConfirmBlock): string {
  switch (block) {
    case "noEndpoint":
      return t("app.setup.notify.relayNoEndpoint");
    case "unsaved":
      return t("app.setup.notify.testNoRecipient");
    case "sameSuppressed":
      return t("app.setup.notify.relayOtherAddress");
  }
}

/** Setningen under brikka: hva tilstanden BETYR, med adressen i seg. */
function stepDescription(view: RelayView): string {
  switch (view.step) {
    case "unavailable":
      return t("app.setup.notify.relayNoEndpoint");
    case "none":
      return t("app.setup.notify.relayNoneDesc");
    case "pending":
      return tf("app.setup.notify.relayPendingDesc", { address: view.address });
    case "confirmed":
      return tf("app.setup.notify.relayConfirmedDesc", {
        address: view.address,
      });
    case "suppressed":
      return t("app.setup.notify.relaySuppressedDesc");
  }
}

/**
 * Adressen — det ene skjemaet på denne siden med Lagre/Avbryt, og reléets
 * tilstandsmaskin rett under det.
 *
 * «Avbryt» ANGRER her, i motsetning til de tre lagre-knappene i det gamle
 * skallet: utkastet er en egen kopi som ingenting utenfor skjemaet ser før
 * `save()`.
 *
 * ## Hvorfor knappene bor i DETTE kortet
 *
 * Fordi de handler om adressen som står i feltet over. En «Bekreft
 * e-postadressen» oppe ved bryteren ville stått uten adressen den bekrefter,
 * og en frivillig ville ikke sett sammenhengen mellom de to.
 *
 * ## Sperren på «Send på nytt»
 *
 * Endepunktet har ti minutter mellom to bekreftelses-e-poster, og svarer 429
 * innenfor dem — som utboksen backer av og prøver igjen på, så ingenting går
 * tapt. Men en knapp som ser trykkbar ut og bare fører til en usynlig kø er en
 * knapp som ikke svarer. Så skjermen holder samme frist LOKALT (fra klikket,
 * ikke fra endepunktets klokke — vi har ikke den, og en frist som er litt for
 * lang er en frist som aldri overrasker) og sier hvor lenge det er igjen.
 */
function AddressCard() {
  const s = settings.value;
  const facts = emailFacts.value;
  const relay = relayFacts.value;
  const { receipt, show: showReceipt, reset: resetReceipt } = useReceipt();
  const [error, setError] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [lastResendAt, setLastResendAt] = useState<number | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const form = useDraftForm(
    () => ({ address: s.emailAddress ?? "" }),
    async (value) => {
      patchSettings({ emailAddress: value.address.trim() });
      const ok = await saveSettingsDebounced(120);
      if (!ok) toast("error", t("general.saveFailed"));
      return ok;
    },
  );

  const address = form.draft.address.trim();
  const invalid = address.length > 0 && !EMAIL_RE.test(address);
  const saved = (s.emailAddress ?? "").trim();
  const view = relayView({
    facts: relay,
    savedAddress: saved,
    dirty: form.dirty,
    lastResendAt,
    now,
  });

  // Klokka går bare mens noe faktisk teller ned. En evig `setInterval` på en
  // innstillingsside ville malt hele treet på nytt hvert sekund for ingenting.
  const cooling = view.resendWaitMs > 0;
  useEffect(() => {
    if (!cooling) return;
    const id = setInterval(() => setNow(Date.now()), COOLDOWN_TICK_MS);
    return () => clearInterval(id);
  }, [cooling]);

  const blocked = emailBlockReason(
    facts ?? {
      featureBuilt: false,
      smtpConfigured: false,
      smtpPasswordAvailable: false,
    },
    !!s.emailAddress,
    view.transport,
  );

  async function save(): Promise<void> {
    if (invalid) {
      setError(t("app.setup.notify.invalid"));
      return;
    }
    setError(null);
    showReceipt("saving");
    showReceipt((await form.save()) ? "saved" : "failed");
  }

  /**
   * Én vei gjennom de tre relé-knappene: kall, les status på nytt, si fra.
   *
   * Feilen SVELGES ikke og oversettes ikke bort: bakendens koder
   * (`relay_invalid_address`, `relay_no_endpoint`, `relay_not_confirmed`) er
   * det eneste som skiller «adressen din har en skrivefeil» fra «denne
   * utgaven kan ikke dette», og en frivillig som får «noe gikk galt» leter
   * begge steder.
   */
  async function relayAction(
    run: () => Promise<unknown>,
    okMessage: string,
  ): Promise<void> {
    if (busy) return;
    setBusy(true);
    try {
      await run();
      toast("success", okMessage);
    } catch (err) {
      const code = errorCode(err);
      toast(
        "error",
        code === "relay_invalid_address"
          ? t("app.setup.notify.invalid")
          : code === "relay_no_endpoint"
            ? t("app.setup.notify.relayNoEndpoint")
            : tf("app.setup.notify.relayFailed", {
                err: err instanceof Error ? err.message : String(err),
              }),
      );
    } finally {
      // Alltid, også etter en feil: bakenden kan ha rukket å skrive raden før
      // den kastet, og en skjerm som viser noe annet enn basen er hvordan en
      // «Bekreft» blir trykket to ganger.
      await refreshRelayFacts();
      setBusy(false);
    }
  }

  const confirm = () =>
    relayAction(
      () => window.api.relaySubscribe(saved),
      tf("app.setup.notify.relaySubscribed", { to: saved }),
    );

  const resend = () => {
    setLastResendAt(Date.now());
    setNow(Date.now());
    return relayAction(
      () => window.api.relayResend(),
      t("app.setup.notify.relayResent"),
    );
  };

  const unsubscribe = () =>
    relayAction(
      () => window.api.relayUnsubscribe(),
      t("app.setup.notify.relayUnsubscribed"),
    );

  /**
   * «Send en test» går gjennom den AKTIVE kanalen.
   *
   * Bekreftet relé ⇒ `relaySendTest` (som bakenden gjør til en køet melding
   * til den bekreftede adressen). Ellers SMTP-stien, uendret. Å teste den ene
   * veien mens den andre er den som faktisk brukes ville vært en grønn test
   * for en kanal ingen varsler går gjennom.
   */
  async function sendTest(): Promise<void> {
    if (testing) return;
    const to = view.transport ? view.address : saved;
    if (!to) return;
    setTesting(true);
    try {
      const result = view.transport
        ? await window.api.relaySendTest()
        : await window.api.testEmail({
            recipient: to,
            language: locale.peek(),
            host: s.emailSmtp,
            port: s.emailSmtpPort,
            user: s.emailSmtpUser,
            // Tomt betyr «utled den» i bakenden, som er det hver eksisterende
            // konfigurasjon gjør.
            from: (s.emailSmtpFrom ?? "").trim() || undefined,
          });
      toast(
        result.ok ? "success" : "error",
        result.ok
          ? tf("app.setup.notify.testSent", { to })
          : tf("app.setup.notify.testFailed", { err: result.error ?? "" }),
      );
    } finally {
      setTesting(false);
    }
  }

  return (
    <Card testId="notify-address-card">
      <SettingRow
        label={t("app.setup.notify.to")}
        description={t("app.setup.notify.toDesc")}
        receipt={receipt}
        error={error}
        testId="notify-address"
      >
        {(ids) => (
          <TextField
            value={form.draft.address}
            type="email"
            placeholder={t("app.setup.notify.toPlaceholder")}
            invalid={invalid}
            onInput={(next) => {
              setError(null);
              resetReceipt();
              form.set({ address: next });
            }}
            labelId={ids.labelId}
            describedBy={ids.describedBy}
            testId="notify-address-control-input"
          />
        )}
      </SettingRow>

      <SettingRow
        label={t("app.setup.notify.relayLabel")}
        description={stepDescription(view)}
        testId="notify-relay"
      >
        <Chip tone={STEP_TONE[view.step]} testId="notify-relay-state">
          {stepChip(view.step)}
        </Chip>
        {view.queued ? (
          <span class={styles.hint}>{t("app.setup.notify.relayQueued")}</span>
        ) : null}
        {view.showConfirm ? (
          <Button
            variant="primary"
            busy={busy}
            disabled={view.confirmBlock !== null}
            disabledReason={
              view.confirmBlock
                ? confirmBlockReason(view.confirmBlock)
                : undefined
            }
            testId="notify-relay-confirm"
            onClick={() => void confirm()}
          >
            {t("app.setup.notify.relayConfirm")}
          </Button>
        ) : null}
        {view.showResend ? (
          <Button
            variant="secondary"
            busy={busy}
            disabled={view.resendWaitMs > 0}
            disabledReason={tf("app.setup.notify.relayResendWait", {
              n: resendWaitMinutes(view.resendWaitMs),
            })}
            testId="notify-relay-resend"
            onClick={() => void resend()}
          >
            {t("app.setup.notify.relayResend")}
          </Button>
        ) : null}
        {view.showUnsubscribe ? (
          <Button
            variant="ghost"
            busy={busy}
            testId="notify-relay-unsubscribe"
            onClick={() => void unsubscribe()}
          >
            {t("app.setup.notify.relayUnsubscribe")}
          </Button>
        ) : null}
      </SettingRow>

      <div class={styles.footer}>
        <Button
          variant="primary"
          disabled={!form.dirty}
          disabledReason={t("app.setup.notify.nothingToSave")}
          testId="notify-save"
          onClick={() => void save()}
        >
          {t("app.setup.save")}
        </Button>
        <Button
          variant="secondary"
          disabled={!form.dirty}
          disabledReason={t("app.setup.notify.nothingToSave")}
          testId="notify-cancel"
          onClick={() => {
            setError(null);
            resetReceipt();
            form.cancel();
          }}
        >
          {t("app.setup.cancel")}
        </Button>
        <Button
          variant="ghost"
          busy={testing}
          disabled={blocked !== null}
          disabledReason={
            blocked === "noFeature"
              ? t("app.setup.notify.gateNoFeature")
              : blocked === "noRecipient"
                ? t("app.setup.notify.testNoRecipient")
                : t("app.setup.notify.gateSmtp")
          }
          testId="notify-test"
          onClick={() => void sendTest()}
        >
          {t("app.setup.notify.test")}
        </Button>
      </div>
    </Card>
  );
}
