/**
 * PageShell — skinnen, de tre destinasjonene, tannhjulet og setningen nederst.
 *
 * ⚠️ MIDLERTIDIG FORM. D3 PR2 river skinnen og setter destinasjonene som ikoner
 * NEDERST à la DaVinci Resolve, med logoen i en slank topplinje. Denne fila er
 * derfor den gamle skinnen med en destinasjon til i den — ikke fordi formen er
 * riktig, men fordi rekkefølgen på PR-ene er sidene først og skallet etterpå,
 * og et halvt skall i mellomtiden ville betydd at ingen av dem kunne merges
 * alene.
 *
 * ## Tre steder og et tannhjul, ikke fem sider og åtte faner
 *
 * Legacy har `home`, `schedule`, `search`, `settings` og `editor`, pluss fem
 * faner inne i innstillinger. En frivillig som aldri har sett appen skal ikke
 * måtte vite hvilken av dem «filnavn» bor på. Skinnen har tre knapper —
 * Opptak · Redigering · Eksportering, i den rekkefølgen man gjør dem — og
 * ruteren (`app/router/router.ts`) oversetter alt det gamle til dem.
 *
 * ## Hvorfor Innstillinger IKKE er en av dem lenger (D2)
 *
 * Fram til D2 sto Oppsett som destinasjon nummer tre, ved siden av Opptak og
 * Bibliotek. Det gjorde innstillinger til et LIKEVERDIG sted — noe man går til
 * like ofte som man tar opp — og det er ikke sant: en frivillig setter opp
 * appen én gang og tar opp hver søndag. Tannhjulet står derfor nederst, over
 * statuslinjen, der den gamle appen hadde det, og der et tannhjul står i alt
 * annet en frivillig bruker.
 *
 * RUTEN `setup` lever videre uendret. Det er bare plasseringen i skinnen som
 * flyttet: `data-testid="nav-setup"` og `aria-current` står på tannhjulet, så
 * alt som spør skinnen «hvor er jeg?» får samme svar som før, og
 * `[data-testid^="nav-"]` teller fortsatt tre.
 *
 * ## `data-tauri-drag-region`
 *
 * Attributtet må stå EKSAKT slik på skinnens rot: det er Tauri som leser det,
 * og det er den eneste måten vinduet kan dras på når tittellinjen er skjult.
 * Uten det blir appen et vindu man ikke kan flytte — en feil ingen tester
 * finner, fordi alle tester klikker og ingen drar.
 *
 * Knappene inni bærer `data-tauri-drag-region="false"`, ellers ville et klikk
 * på en destinasjon startet et vindusdrag i stedet for å navigere.
 *
 * ## Rutebytte flytter fokus
 *
 * Ny side ⇒ `#main` rulles til topp og fokus settes på `<h1>`. Uten det blir en
 * tastaturbruker stående i skinnen og må tabbe gjennom hele navigasjonen på
 * nytt for hver side, og en skjermleserbruker får ingen beskjed om at siden
 * skiftet i det hele tatt. `tabIndex={-1}` på overskriften er det som gjør den
 * fokuserbar uten å legge seg i tabrekkefølgen.
 *
 * ## Statuslinjen
 *
 * Én av fem setninger, valgt av `statusLine()` (ren, tabelltestet). Skinnen
 * gjør ingen prioritering selv — den henter tilstanden fra signalene og maler
 * svaret.
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

/**
 * Destinasjonene i navigasjonen. `setup` er IKKE med — den har tannhjulet
 * nederst, se toppen av fila. Ruten finnes fortsatt; det er bare knappen som
 * har flyttet.
 */
const PAGES: readonly Page[] = ["record", "edit", "export"];

/**
 * Produktnavnet. En KONSTANT og ikke tekst i treet: navnet oversettes ikke —
 * det heter SundayRec på alle sju språk — men en gate som må vurdere hver
 * bokstavsekvens i JSX kan ikke vite det, og skal ikke måtte gjette. Ett navn,
 * ett sted.
 */
const PRODUCT = "SundayRec";

/** Ikonene fra canvasen. Rent dekorative — etiketten står ved siden av. */
const ICONS: Record<Page, ComponentChildren> = {
  record: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <circle cx="12" cy="12" r="9" />
      <circle cx="12" cy="12" r="3.5" fill="currentColor" stroke="none" />
    </svg>
  ),
  /*
   * REDIGERING: to spor over en linje — tidslinja man klipper i, og ikke en
   * saks. En saks tegner HANDLINGEN å fjerne noe; sporene tegner STEDET, og
   * det er stedet destinasjonen er.
   */
  edit: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <rect x="3" y="4" width="18" height="5" rx="1.5" />
      <rect x="3" y="11" width="18" height="5" rx="1.5" />
      <path d="M6 20h12" />
    </svg>
  ),
  /*
   * EKSPORTERING: en pil UT av en boks. Ikke en diskett og ikke en sky —
   * eksporten skriver en ny fil ved siden av opptaket, og pilen ut er det ene
   * tegnet som betyr det samme i alle programmene en frivillig ellers bruker.
   */
  export: (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M12 3v11" />
      <path d="M8.5 6.5 12 3l3.5 3.5" />
      <path d="M4 14v5a1.5 1.5 0 0 0 1.5 1.5h13A1.5 1.5 0 0 0 20 19v-5" />
    </svg>
  ),
  /*
   * Et EKTE tannhjul, ikke skyvekontrollene fra før D2. Skyvekontroller er et
   * mikserbord for den som allerede vet hva de gjør; tannhjulet er tegnet
   * ingen trenger å lære — og det er det som står i den gamle appen, i
   * menylinjen og i alt annet en frivillig bruker.
   *
   * Geometrien er regnet ut, ikke lånt: senter (12,12), nav r 2,6, ring r 6,4
   * og åtte tenner fra r 6,4 til r 9,2 for hver 45°. Diagonalene er
   * 6,4·cos45° ≈ 4,5 og 9,2·cos45° ≈ 6,5 — derav 7,5/16,5 og 5,5/18,5.
   */
  setup: (
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

  const church = (s.churchName ?? "").trim();
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
      <nav
        // EKSAKT dette attributtet — se toppen av fila.
        data-tauri-drag-region
        aria-label={t("app.rail.label")}
        data-testid="rail"
        class={styles.rail}
      >
        <div class={styles.logo}>
          <AppLogo size={28} />
          <b class={styles.logoText}>{PRODUCT}</b>
        </div>

        <div
          data-testid="rail-church"
          class={`${styles.church} ${church ? "" : styles.churchUnset}`}
        >
          {church || t("app.rail.churchUnset")}
        </div>

        <div class={styles.nav}>
          {PAGES.map((page) => (
            <button
              key={page}
              type="button"
              data-tauri-drag-region="false"
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
        </div>

        {/*
          Tannhjulet. Nederst, over statuslinjen — se toppen av fila. Det er
          `.navItem` som alle andre (samme høyde, samme hover, samme `.on`);
          det er PLASSERINGEN som sier at det ikke er en destinasjon.
        */}
        <div class={styles.settingsItem}>
          <button
            type="button"
            data-tauri-drag-region="false"
            data-testid="nav-setup"
            aria-current={current.page === "setup" ? "page" : undefined}
            class={`${styles.navItem} ${current.page === "setup" ? styles.on : ""}`}
            onClick={() => navigate("setup")}
          >
            <span aria-hidden="true" class={styles.navIcon}>
              {ICONS.setup}
            </span>
            <span>{t("app.page.setup")}</span>
          </button>
        </div>

        <div class={styles.status}>
          <div
            data-testid="status-line"
            data-status={status.kind}
            class={styles.statusLine}
          >
            <StatusDot tone={status.tone} testId="status-dot" />
            <span data-testid="status-text">{statusText}</span>
          </div>
          {appVersion.value ? (
            <div data-testid="app-version" class={styles.version}>
              {PRODUCT} {appVersion.value}
            </div>
          ) : null}
        </div>
      </nav>

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
    </div>
  );
}
