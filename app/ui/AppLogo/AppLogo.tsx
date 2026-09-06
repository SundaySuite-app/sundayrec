/**
 * AppLogo — merket øverst til venstre i topplinja. Den gamle appens logo,
 * tilbake.
 *
 * ## Hvorfor tegningen står her, ordrett
 *
 * Det nye skallet malte en gul boks med en «S» i (`PageShell`s `.mark`). Den
 * var en plassholder som overlevde inn i en beta: eieren kjente ikke igjen sin
 * egen app, fordi merket hun har brukt i to år ikke var der. Tegningen under er
 * derfor hentet ORDRETT fra den utsendte appen —
 * `git show d982012:legacy/renderer/index.html`, linje 20–38 — og ikke tegnet
 * på nytt etter hukommelsen. En logo som er «nesten» den samme er den ene
 * formen for feil ingen melder fra om, men alle ser.
 *
 * Inline SVG og ikke `<img src="…">`: merket skal arve størrelsen fra skallet,
 * males i første frame (ingen ekstra forespørsel som kan komme etter at
 * vinduet er tegnet), og det gjør at det ikke finnes en binærfil til som må
 * holdes i takt med `src-tauri/app-icon.svg`.
 *
 * ## ⚠️ `srlogo-`-prefikset er en KOLLISJONSVAKT, ikke pynt
 *
 * `<defs>`-id-er er globale i dokumentet, ikke lokale for sitt eget
 * `<svg>`-element. `src-tauri/app-icon.svg` tegner nøyaktig det samme merket
 * med de generiske id-ene `bg`, `glow`, `gold` og `clip` — så to inline-kopier
 * i samme dokument ville pekt `url(#gold)` på hverandres gradient, og den som
 * ble parset sist ville vunnet. Legacy-skallet prefikset dem `srlogo-*` av
 * denne grunnen, og prefikset følger med hit. Endres det, må BEGGE stedene
 * (`fill="url(#…)"` og `clip-path="url(#…)"`) endres samtidig — ellers blir
 * merket en svart firkant, og bare noen ganger.
 *
 * ## ⚠️ Heksadesimalfargene er et DOKUMENTERT unntak fra fargegaten
 *
 * `scripts/check-app-css-tokens.mjs` sier at en farge finnes ÉN gang, i
 * `app/styles/tokens.css`. Gaten leser `.css` under `app/`, og literalene her
 * står i TSX — de passerer den altså uten å bli sett. Det er med vilje, og
 * dette avsnittet er begrunnelsen som ellers hadde manglet:
 *
 * Merkevaren er ikke en palett. `--gold` i tokens.css er appens aksentfarge og
 * skal kunne justeres av eieren; marineblå #2A4E92→#172F5E og gull
 * #F2D58A→#EBB84B ER logoen, på samme måte som kurvene er det. En logo som
 * skifter farge når noen justerer et tema er ikke lenger en logo. Derfor bor
 * de i tegningen, ikke i ordboken — og derfor står de her og ingen andre
 * steder i `app/`.
 *
 * ## Ingen `aria-label`
 *
 * Merket er `aria-hidden`. Produktnavnet står som ekte tekst rett ved siden av
 * det i topplinja, så en etikett her ville fått en skjermleser til å si
 * «SundayRec, SundayRec». (Legacy hadde `aria-label="SundayRec logo"` fordi
 * den ikke hadde noe annet valg — teksten der var et `data-i18n`-span.)
 */

import styles from "./AppLogo.module.css";

export interface AppLogoProps {
  /** Kantlengden i piksler. Topplinja ber om 22 (D3 — skinnen ba om 28, og
   *  standardverdien er fortsatt den, fordi den er tegningens eget mål). */
  size?: number;
}

export function AppLogo({ size = 28 }: AppLogoProps) {
  return (
    <span
      aria-hidden="true"
      data-testid="app-logo"
      class={styles.logo}
      style={{ width: size, height: size }}
    >
      <svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg">
        <defs>
          <linearGradient id="srlogo-bg" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stop-color="#2A4E92" />
            <stop offset="1" stop-color="#172F5E" />
          </linearGradient>
          <radialGradient id="srlogo-glow" cx="0.26" cy="0.2" r="0.95">
            <stop offset="0" stop-color="#ffffff" stop-opacity="0.12" />
            <stop offset="0.55" stop-color="#ffffff" stop-opacity="0" />
          </radialGradient>
          <linearGradient id="srlogo-gold" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stop-color="#F2D58A" />
            <stop offset="1" stop-color="#EBB84B" />
          </linearGradient>
          <clipPath id="srlogo-clip">
            <rect width="1024" height="1024" rx="228" />
          </clipPath>
        </defs>
        <g clip-path="url(#srlogo-clip)">
          <rect width="1024" height="1024" fill="url(#srlogo-bg)" />
          <rect width="1024" height="1024" fill="url(#srlogo-glow)" />
          <path
            d="M 701.3 375.6 A 161.3 161.3 0 0 1 701.3 583.0"
            fill="none"
            stroke="#EBB84B"
            stroke-width="15"
            stroke-linecap="round"
          />
          <path
            d="M 322.7 375.6 A 161.3 161.3 0 0 0 322.7 583.0"
            fill="none"
            stroke="#EBB84B"
            stroke-width="15"
            stroke-linecap="round"
          />
          <path
            d="M 766.5 301.6 A 270.8 270.8 0 0 1 766.5 657.0"
            fill="none"
            stroke="#EBB84B"
            stroke-width="15"
            stroke-linecap="round"
          />
          <path
            d="M 257.4 301.6 A 270.8 270.8 0 0 0 257.4 657.0"
            fill="none"
            stroke="#EBB84B"
            stroke-width="15"
            stroke-linecap="round"
          />
          <rect
            x="479.5"
            y="320.0"
            width="65.0"
            height="425.8"
            rx="6.2"
            fill="url(#srlogo-gold)"
          />
          <rect
            x="376.8"
            y="437.6"
            width="270.4"
            height="65.0"
            rx="6.2"
            fill="url(#srlogo-gold)"
          />
        </g>
      </svg>
    </span>
  );
}
