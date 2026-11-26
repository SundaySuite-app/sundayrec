/**
 * PageShell — toppfeltet, siden, og en slank bunnlinje («Én flate»).
 *
 * ## Toppfeltet (E1)
 *
 * Venstre: det rene gullkorset (`AppLogo bare`, uten ordmerke). Midten: to
 * ikonfaner — TA OPP og OPPTAK — 52 px brede piller der den aktive har hevet
 * bakgrunn og gullikon; navnet står som `aria-label` og `title`. Høyre:
 * tannhjulet som åpner INNSTILLINGER. PLAN er ikke lenger en fane (ruteren
 * sender `plan` til TA OPP med stripa åpen, se E2).
 *
 * Mens et opptak går skjules fanene og tannhjulet; midten viser en blinkende
 * rød prikk og «Tar opp». Det er det samme `isRecording`-signalet som resten
 * av skallet bruker.
 *
 * `data-testid="nav-<side>"` og `aria-current` står på knappen for siden
 * (også tannhjulet: `nav-settings`), så alt som spør skallet «hvor er jeg?»
 * får svar fra samme sted som før.
 *
 * Bunnlinjen (56 px) har statuslinjen til venstre og versjonen til høyre.
 *
 * ## Rutebytte flytter fokus
 *
 * Ny side ⇒ `#main` rulles til topp og fokus settes på `<h1>`. Uten det blir en
 * tastaturbruker stående i navigasjonen og må tabbe gjennom hele den på nytt
 * for hver side, og en skjermleserbruker får ingen beskjed om at siden skiftet
 * i det hele tatt. `tabIndex={-1}` på overskriften er det som gjør den
 * fokuserbar uten å legge seg i tabrekkefølgen.
 *
 * ## `data-tauri-drag-region` — attributtet flyttet, kontrakten ikke
 *
 * Attributtet må stå EKSAKT slik, og nå på TOPPLINJENS rot: det er Tauri som
 * leser det, og det er den eneste måten vinduet kan dras på når tittellinjen er
 * skjult. Uten det blir appen et vindu man ikke kan flytte — en feil ingen
 * tester finner, fordi alle tester klikker og ingen drar.
 *
 * Fra E1 har topplinja knapper (fanene og tannhjulet). Tauri leser attributtet
 * bare på selve elementet som treffes (ikke på barna), så knappene trenger
 * ingen unntak — og attributtet står fortsatt på ÉN rot. Derfor er logoen,
 * fanene og tannhjulet direkte rutenettbarn av `<header>` (ingen omsluttende
 * `div`er som ville stjålet dra-flaten), og det tomme mellom dem er selve
 * dra-sonen. Bunnlinja er ikke en dra-sone i det hele tatt.
 *
 * ## Rutebytte flytter fokus
 *
 * Ny side ⇒ `#main` rulles til topp og fokus settes på `<h1>`. Uten det blir en
 * tastaturbruker stående i navigasjonen og må tabbe gjennom hele den på nytt
 * for hver side, og en skjermleserbruker får ingen beskjed om at siden skiftet
 * i det hele tatt. `tabIndex={-1}` på overskriften er det som gjør den
 * fokuserbar uten å legge seg i tabrekkefølgen.
 *
 * ## Statuslinjen
 *
 * Én av fem setninger, valgt av `statusLine()` (ren, tabelltestet). Skallet
 * gjør ingen prioritering selv — det henter tilstanden fra signalene og maler
 * svaret. Den flyttet fra skinnens bunn til bunnlinjas venstre ende; matingen
 * er ordrett den samme.
 *
 * ⚠️ Katalognøklene heter fortsatt `app.rail.*`. Nøkler er identiteter, ikke
 * beskrivelser: å døpe dem om ville kostet to språkfiler og paritetslista for
 * å endre et ord ingen bruker ser.
 */

import type { ComponentChildren } from "preact";
import { useEffect, useRef } from "preact/hooks";

import { locale, t, tDyn, tf } from "../../i18n";
import { navigate, route } from "../../router/router";
import { appVersion } from "../../state/app-info";
import { audioDevices, soundChosen } from "../../state/devices";
import { currentRoomMinutes } from "../../state/disk";
import { nextRecording } from "../../state/next-recording";
import { isRecording } from "../../state/recording";
import { settings } from "../../state/settings";
import { formatNextWhen, statusLine } from "../../state/status-line";
import { AppLogo } from "../AppLogo/AppLogo";
import { Icon } from "../Icon/Icon";
import { StatusDot } from "../StatusDot/StatusDot";
import styles from "./PageShell.module.css";

/** Fanene i midten av toppfeltet, venstre→høyre. Innstillinger er tannhjulet. */
const TABS = [
  { page: "record", icon: "record" },
  { page: "recordings", icon: "recordings" },
] as const;

/**
 * Produktnavnet. En KONSTANT og ikke tekst i treet: navnet oversettes ikke —
 * det heter SundayRec på begge språk — men en gate som må vurdere hver
 * bokstavsekvens i JSX kan ikke vite det, og skal ikke måtte gjette. Ett navn,
 * ett sted.
 */
const PRODUCT = "SundayRec";

export interface PageShellProps {
  children: ComponentChildren;
  /**
   * Overskriften, når siden handler om noe smalere enn destinasjonen.
   *
   * OPPSETT er tre skjermer dypt: ruten heter «Innstillinger» (D2 — den het
   * «Oppsett» til og med v0.16.0-beta.1), men den skjermen man står PÅ heter
   * «Hvilken lyd?». En `<h1>` som sa det samme på alle seks ville vært det ene
   * ordet som aldri hjelper — og siden fokus
   * flyttes hit ved hvert rutebytte, er det også det første en
   * skjermleserbruker hører. Utelatt = destinasjonens eget navn.
   */
  heading?: string;
}

export function PageShell({ children, heading }: PageShellProps) {
  const current = route.value;
  const recording = isRecording.value;
  const s = settings.value;
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const mainRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    // Ny side ELLER ny fane: øverst, og fokus på overskriften. Fanen teller
    // med fordi de fem oppsett-spørsmålene er egne skjermer — å åpne «Hvilken
    // lyd?» uten at fokus flyttet seg ville latt en tastaturbruker stå igjen
    // på knappen på en side som ikke finnes lenger.
    mainRef.current?.scrollTo?.({ top: 0 });
    headingRef.current?.focus();
  }, [current.page, current.tab]);

  const next = nextRecording.value.next;
  const status = statusLine({
    isRecording: isRecording.value,
    // «Valgt» betyr valgt OG til stede — se `soundChosen` i state/devices.ts.
    soundChosen: soundChosen(s, audioDevices.value),
    roomMinutes: currentRoomMinutes(),
    nextAtMs: next?.atMs ?? null,
  });
  const statusText =
    status.kind === "next" && next
      ? tf("app.status.next", { when: formatNextWhen(next.atMs, locale.value) })
      : tDyn("app.status", status.kind);

  return (
    <div class={styles.page}>
      <header
        // EKSAKT dette attributtet, og EKSAKT her — se toppen av fila.
        data-tauri-drag-region
        data-testid="topbar"
        class={styles.topbar}
      >
        <AppLogo bare size={42} />

        <nav aria-label={t("app.rail.label")} class={styles.tabs}>
          {recording ? (
            <span data-testid="topbar-recording" class={styles.recording}>
              <span aria-hidden="true" class={styles.recDot} />
              {t("app.status.rec")}
            </span>
          ) : (
            TABS.map(({ page, icon }) => (
              <button
                key={page}
                type="button"
                data-testid={`nav-${page}`}
                aria-label={tDyn("app.page", page)}
                title={tDyn("app.page", page)}
                aria-current={current.page === page ? "page" : undefined}
                class={`${styles.tab} ${current.page === page ? styles.on : ""}`}
                onClick={() => navigate(page)}
              >
                <Icon name={icon} size={24} />
              </button>
            ))
          )}
        </nav>

        {recording ? null : (
          <button
            type="button"
            data-testid="nav-settings"
            aria-label={tDyn("app.page", "settings")}
            title={tDyn("app.page", "settings")}
            aria-current={current.page === "settings" ? "page" : undefined}
            class={`${styles.gear} ${current.page === "settings" ? styles.on : ""}`}
            onClick={() => navigate("settings")}
          >
            <Icon name="gear" size={22} />
          </button>
        )}
      </header>

      {/*
        Ruten som ATTRIBUTTER, ikke som synlig tekst. S1a viste `tab`, `anchor`
        og `firstRun` som små avsnitt i treet fordi det ikke fantes noe annet å
        se; det var feilsøkingstekst i en app en frivillig skal lese. Nå er de
        det de alltid var — noe e2e kan spørre om, og som ingen ser.
      */}
      <main
        id="main"
        ref={mainRef}
        data-testid="main"
        data-page={current.page}
        data-tab={current.tab}
        data-panel={current.panel}
        data-anchor={current.anchor}
        data-first-run={current.firstRun ? "true" : undefined}
        class={styles.main}
      >
        <h1
          ref={headingRef}
          tabIndex={-1}
          data-testid="app-heading"
          class={styles.h1}
        >
          {heading ?? tDyn("app.page", current.page)}
        </h1>
        {children}
      </main>

      {/*
        Bunnlinja. Tre felt i et `1fr auto 1fr`-rutenett, som er det som holder
        destinasjonene SENTRERT uansett hvor lang setningen til venstre er.
        Ingen dra-sone: hele båndet er knapper og tekst man skal kunne treffe.
      */}
      <footer data-testid="bottombar" class={styles.bottombar}>
        <div class={styles.statusCell}>
          <div
            data-testid="status-line"
            data-status={status.kind}
            class={styles.statusLine}
          >
            <StatusDot tone={status.tone} testId="status-dot" />
            <span data-testid="status-text" class={styles.statusText}>
              {statusText}
            </span>
          </div>
        </div>

        <div class={styles.tail}>
          {appVersion.value ? (
            <div data-testid="app-version" class={styles.version}>
              {PRODUCT} {appVersion.value}
            </div>
          ) : null}
        </div>
      </footer>
    </div>
  );
}
