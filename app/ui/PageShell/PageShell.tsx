/**
 * PageShell — en slank topplinje, siden, og en bunnlinje à la DaVinci Resolve.
 *
 * ## Skinnen er borte, og hvorfor
 *
 * Fram til D3 lå navigasjonen i en 208 px bred venstreskinne. Den kostet en
 * sjettedel av bredden på en 1180 px skjerm — plass kontrollrommet trenger til
 * innhold som faktisk sier noe. Det som var i skinnen bor nå i to bånd som til
 * sammen er hundre piksler høye i stedet for to hundre piksler brede:
 *
 *   TOPPLINJEN  (44 px) merket og produktnavnet — og dra-sonen.
 *   BUNNLINJEN  (56 px) statuslinjen til venstre, de fire sidene sentrert,
 *               versjonen til høyre.
 *
 * ## Fire sider (F5)
 *
 * Ta opp · Opptak · Plan · Innstillinger, venstre→høyre. Innstillinger er
 * ikke lenger et eget tannhjul ytterst til høyre — den er den fjerde knappen i
 * samme gruppe, med tannhjulet som ikon. Ruten heter `settings`;
 * `data-testid="nav-<side>"` og `aria-current` står på knappen for siden, så
 * alt som spør skallet «hvor er jeg?» får svar fra samme sted som før.
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
 * Topplinja har med vilje INGEN interaktive barn. Det er derfor ingen av dem
 * bærer `data-tauri-drag-region="false"` lenger: unntaket fantes fordi
 * destinasjonene lå inne i dra-sonen, og et klikk på dem ellers ville startet
 * et vindusdrag i stedet for å navigere. Kommer det en knapp hit en dag, må den
 * bære `data-tauri-drag-region="false"` — og bunnlinja, som er full av knapper,
 * er ikke en dra-sone i det hele tatt.
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
import { navigate, route, type Page } from "../../router/router";
import { appVersion } from "../../state/app-info";
import { audioDevices, soundChosen } from "../../state/devices";
import { currentRoomMinutes } from "../../state/disk";
import { nextRecording } from "../../state/next-recording";
import { isRecording } from "../../state/recording";
import { settings } from "../../state/settings";
import { formatNextWhen, statusLine } from "../../state/status-line";
import { AppLogo } from "../AppLogo/AppLogo";
import { StatusDot } from "../StatusDot/StatusDot";
import styles from "./PageShell.module.css";

/** Sidene i navigasjonen, venstre→høyre. */
const PAGES: readonly Page[] = ["record", "recordings", "plan", "settings"];

/**
 * Produktnavnet. En KONSTANT og ikke tekst i treet: navnet oversettes ikke —
 * det heter SundayRec på begge språk — men en gate som må vurdere hver
 * bokstavsekvens i JSX kan ikke vite det, og skal ikke måtte gjette. Ett navn,
 * ett sted.
 */
const PRODUCT = "SundayRec";

/**
 * Ikonene fra canvasen. Rent dekorative — etiketten står UNDER hver av dem.
 *
 * Typen er `Record<Page, …>` med vilje: den er kompileringsgaten som gjør en ny
 * destinasjon umulig å legge til uten å tegne den.
 */
const ICONS: Record<Page, ComponentChildren> = {
  record: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <circle cx="12" cy="12" r="9" />
      <circle cx="12" cy="12" r="3.5" fill="currentColor" stroke="none" />
    </svg>
  ),
  /*
   * OPPTAK: to spor over en linje — lista over det man har tatt opp, og
   * tidslinja man klipper i.
   */
  recordings: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <rect x="3" y="4" width="18" height="5" rx="1.5" />
      <rect x="3" y="11" width="18" height="5" rx="1.5" />
      <path d="M6 20h12" />
    </svg>
  ),
  /* PLAN: en kalender — en uke med en markert dag. */
  plan: (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <rect x="3.5" y="5" width="17" height="15" rx="2" />
      <path d="M3.5 10h17M8 3v4M16 3v4" />
      <circle cx="8.5" cy="14.5" r="1" fill="currentColor" stroke="none" />
    </svg>
  ),
  /*
   * Et EKTE tannhjul: tegnet ingen trenger å lære — og det som står i
   * menylinjen og i alt annet en frivillig bruker.
   *
   * Geometrien er regnet ut, ikke lånt: senter (12,12), nav r 2,6, ring r 6,4
   * og åtte tenner fra r 6,4 til r 9,2 for hver 45°. Diagonalene er
   * 6,4·cos45° ≈ 4,5 og 9,2·cos45° ≈ 6,5 — derav 7,5/16,5 og 5,5/18,5.
   */
  settings: (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
    >
      <circle cx="12" cy="12" r="6.4" />
      <circle cx="12" cy="12" r="2.6" />
      <path d="M12 5.6V2.8M12 18.4V21.2M5.6 12H2.8M18.4 12H21.2M16.5 7.5L18.5 5.5M7.5 7.5L5.5 5.5M7.5 16.5L5.5 18.5M16.5 16.5L18.5 18.5" />
    </svg>
  ),
};

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
        <AppLogo size={22} />
        <b class={styles.logoText}>{PRODUCT}</b>
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

        <nav aria-label={t("app.rail.label")} class={styles.nav}>
          {PAGES.map((page) => (
            <button
              key={page}
              type="button"
              data-testid={`nav-${page}`}
              aria-current={current.page === page ? "page" : undefined}
              class={`${styles.navItem} ${current.page === page ? styles.on : ""}`}
              onClick={() => navigate(page)}
            >
              <span aria-hidden="true" class={styles.navIcon}>
                {ICONS[page]}
              </span>
              <span>{tDyn("app.page", page)}</span>
            </button>
          ))}
        </nav>

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
