/**
 * DialogHost — flaten under `app/ui/dialog.ts`' kø.
 *
 * ## Hvor den bor, og hvorfor det er viktig
 *
 * Verten monteres som SØSKEN av `#app`, aldri inne i det. Grunnen er `inert`:
 * mens en dialog står åpen skal resten av appen være helt utilgjengelig — ikke
 * klikkbar, ikke fokuserbar, ikke lesbar for en skjermleser. Det oppnås ved å
 * sette `inert` på `#app`, og en dialog som lå inne i `#app` ville slått av seg
 * selv.
 *
 * `inert` settes i en effekt her, og det er trygt nettopp fordi `#app` er
 * MONTERINGSPUNKTET: Preact rendrer barn INN i det og rører aldri elementets
 * egne attributter, så ingen re-render kan stryke det. (Overalt hvor `inert`
 * havner på et element Preact faktisk eier — se `Gate` — er det en JSX-prop,
 * fordi der ville en imperativ skrivning blitt strøket ved neste render,
 * stille og bare noen ganger.)
 *
 * ## Beslutningene er lånt, ikke skrevet på nytt
 *
 * `buildConfirm` fra `@lib/ui/dialog-core` bestemmer knappene: id-ene
 * (`data-dialog-button="ok" | "cancel"`, som et titalls e2e-spec hviler på),
 * variantene, og den ene som betyr noe — på en FARLIG dialog er det AVBRYT som
 * er Enter-valget, og bekreft-knappen er rød SEKUNDÆR. Aldri rød primær:
 * canvasens sett 7. En destruktiv handling skal ikke være ett feiltrykk unna.
 *
 * `nextFocusIndex` fra samme modul er fokusfellen.
 *
 * ## F1-UX1: `aria-describedby` på selve dialogen
 *
 * `aria-labelledby` alene sier hvem dialogen ER («Stoppe opptaket?»), ikke hva
 * den forklarer under det. Uten `aria-describedby` leser en skjermleser
 * tittelen og hopper rett til knappene — brødteksten («1 t 02 min er tatt
 * opp. Stopper du nå, lagres det du har.») finnes i DOM-en, men er aldri
 * KNYTTET til dialogen som en beskrivelse. Nå peker den på `dialog-message`
 * sin id, og bare når meldingen finnes — en referanse til et element som ikke
 * ble rendret er verre enn ingen referanse.
 *
 * ## Fokus tilbake dit brukeren var
 *
 * Vanskeligere enn `document.activeElement` alene: på macOS får en `<button>`
 * IKKE fokus av et klikk (WebKit-oppførsel), så når dialogen åpnes er
 * `activeElement` ofte `<body>` — og fokus ville kommet tilbake til
 * ingensteds. Derfor følger verten med på siste `pointerdown` og bruker det
 * elementet som reserve. Det er den «eksplisitte utløser-ref-en», bare uten at
 * hvert kallsted må huske å sende den.
 */

import { useEffect, useRef } from "preact/hooks";

import { buildAlert, buildConfirm, nextFocusIndex } from "@lib/ui/dialog-core";

import { t } from "../../i18n";
import { Button, type ButtonVariant } from "../Button/Button";
import {
  activeDialog,
  resolveDialog,
  type AlertOpts,
  type ConfirmOpts,
} from "../dialog";
import styles from "./DialogHost.module.css";

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function DialogHost() {
  const pending = activeDialog.value;
  const boxRef = useRef<HTMLDivElement | null>(null);
  const returnTo = useRef<HTMLElement | null>(null);
  /** Sist trykkede kontroll — reserven når `activeElement` er `<body>`. */
  const lastPointer = useRef<HTMLElement | null>(null);
  /** Knappen Enter velger. Settes under render, leses av åpne-effekten. */
  const defaultId = useRef<string>("ok");

  useEffect(() => {
    const onDown = (event: PointerEvent): void => {
      const el = event.target as HTMLElement | null;
      lastPointer.current = el?.closest?.<HTMLElement>(FOCUSABLE) ?? null;
    };
    // Fangstfasen: en håndterer som stopper hendelsen (en meny som lukker seg)
    // skal ikke også gjøre oss blinde for hvem som ble trykket.
    document.addEventListener("pointerdown", onDown, true);
    return () => document.removeEventListener("pointerdown", onDown, true);
  }, []);

  const id = pending?.id ?? null;

  // Åpning og lukking: `inert` på `#app`, fokus inn, fokus tilbake.
  useEffect(() => {
    if (id === null) return;

    const host = document.getElementById("app");
    const active = document.activeElement as HTMLElement | null;
    // `<body>` (og `null`) er ikke et sted å komme tilbake til — se toppen.
    returnTo.current =
      active && active !== document.body && host?.contains(active)
        ? active
        : lastPointer.current;

    host?.setAttribute("inert", "");

    // Fokus inn på standardknappen. `requestAnimationFrame` fordi elementet
    // først finnes etter at Preact har committet denne renderen.
    const raf = requestAnimationFrame(() => {
      const box = boxRef.current;
      const preferred = box?.querySelector<HTMLElement>(
        `[data-dialog-button="${defaultId.current}"]`,
      );
      (preferred ?? box?.querySelector<HTMLElement>(FOCUSABLE))?.focus();
    });

    return () => {
      cancelAnimationFrame(raf);
      host?.removeAttribute("inert");
      // Etter opprydningen, ellers fokuserer vi et element som fortsatt er
      // inert og nettleseren avviser det stille.
      const target = returnTo.current;
      returnTo.current = null;
      if (target?.isConnected) target.focus();
    };
  }, [id]);

  if (!pending) return null;

  const alert = pending.kind === "alert" ? (pending.spec as AlertOpts) : null;
  const spec = alert
    ? buildAlert({ ...alert, okLabel: alert.okLabel ?? t("app.dialog.close") })
    : buildConfirm({
        ...(pending.spec as ConfirmOpts),
        confirmLabel:
          (pending.spec as ConfirmOpts).confirmLabel ?? t("app.dialog.confirm"),
        cancelLabel:
          (pending.spec as ConfirmOpts).cancelLabel ?? t("app.dialog.cancel"),
      });
  const cancelId = spec.buttons.find((b) => b.isCancel)?.id ?? "cancel";
  defaultId.current = spec.buttons.find((b) => b.isDefault)?.id ?? "ok";

  const close = (buttonId: string): void =>
    resolveDialog(pending.id, buttonId === "ok");

  return (
    <div
      class={styles.scrim}
      data-testid="dialog-scrim"
      onMouseDown={(event) => {
        // `mousedown`, ikke `click`: et drag som starter inne i dialogen og
        // slipper på sløret (tekstmarkering) skal ikke lukke den.
        if (event.target === event.currentTarget) close(cancelId);
      }}
    >
      <div
        ref={boxRef}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="app-dialog-title"
        // F1-UX1/W2: peker på brødteksten når det finnes én — ingen id uten
        // et element å peke på, ellers er referansen løs og verre enn ingen.
        // Den preformatterte blokken (`dialog-pre`, telemetriforhåndsvisningen
        // og diagnoserapportene) er IKKE med i referansen, selv de gangene
        // begge står samtidig (`TelemetryRow.showTelemetryPreview` setter
        // både `message` og `preformatted`): en skjermleser som fikk HELE en
        // JSON-nyttelast eller markdown-rapport lest opp som dialogens
        // «beskrivelse» før den engang når knappene ville gjort dialogen
        // tyngre å bruke enn den lange teksten selv rettferdiggjør. Den
        // forblir lesbar i sin egen `<pre>` — bare ikke auto-annonsert.
        aria-describedby={spec.message ? "app-dialog-message" : undefined}
        data-testid="dialog"
        data-danger={spec.danger ? "true" : undefined}
        class={styles.dialog}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            close(cancelId);
            return;
          }
          if (event.key !== "Tab") return;
          const box = boxRef.current;
          if (!box) return;
          const items = Array.from(
            box.querySelectorAll<HTMLElement>(FOCUSABLE),
          );
          if (items.length === 0) return;
          event.preventDefault();
          const next = nextFocusIndex(
            items.length,
            items.indexOf(document.activeElement as HTMLElement),
            event.shiftKey,
          );
          if (next >= 0) items[next].focus();
        }}
      >
        <h2
          id="app-dialog-title"
          data-testid="dialog-title"
          class={styles.title}
        >
          {spec.title}
        </h2>
        {spec.message ? (
          <p
            id="app-dialog-message"
            data-testid="dialog-message"
            class={styles.message}
          >
            {spec.message}
          </p>
        ) : null}
        {/*
          Ordrett, i sin egen rullende blokk. `textContent` og ikke markup: det
          er data fra bakenden, og en `<pre>` er den ene formen som ikke gjør
          om på den.
        */}
        {alert?.preformatted !== undefined ? (
          <pre data-testid="dialog-pre" class={styles.pre}>
            {alert.preformatted}
          </pre>
        ) : null}
        <div class={styles.actions}>
          {spec.buttons.map((button) => (
            <Button
              key={button.id}
              variant={button.variant as ButtonVariant}
              data-dialog-button={button.id}
              testId={`dialog-${button.id}`}
              onClick={() => close(button.id)}
            >
              {button.label}
            </Button>
          ))}
        </div>
      </div>
    </div>
  );
}
