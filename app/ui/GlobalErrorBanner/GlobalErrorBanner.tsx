/**
 * Det globale feilbanneret — den ENESTE leseren av `globalError` (R5).
 *
 * ## Hvorfor den bor i `Overlays` og ikke i `Shell`
 *
 * `globalError` fanges av `installErrorHandlers()` (`state/global-error.ts`)
 * nettopp FORDI et uventet unntak i et Preact-tre stopper rendringen av
 * GRENEN det skjedde i. Sto denne komponenten inne i `Shell` — som rendres i
 * `#app`, se `main.tsx` — ville akkurat den grenen vært den som revnet, og
 * banneret ville aldri kommet opp for feilen det finnes for å vise. `Overlays`
 * er et EGET Preact-tre, montert i `#overlays`, søsken av `#app`: en
 * render-krasj der rører ikke dette treet.
 *
 * ## Rå melding, ikke oversatt
 *
 * `detail` er meldingen SLIK DEN KOM — for oss, ikke for brukeren, se
 * filhodet i `state/global-error.ts`. Bare rammen (tittelen, «Kopier»,
 * krysset) er oversatt. En frivillig som skjermdumper banneret eller trykker
 * «Kopier» og limer det inn i en e-post til dev@ gir oss den ekte
 * feilteksten, ikke en norsk omskrivning av den.
 *
 * ## `--z-critical`
 *
 * Toppen av stabelen — over toast (`--z-toast`), dialog (`--z-modal`) og
 * opptaksoverlegget (`--z-recording`). Appen har ingen god måte å fortsette
 * rendringen på etter et uventet unntak, så dette ER det viktigste som kan
 * stå på skjermen akkurat da.
 */

import { globalError } from "../../state/global-error";
import { t } from "../../i18n";
import { Banner } from "../Banner/Banner";
import { Button } from "../Button/Button";
import styles from "./GlobalErrorBanner.module.css";

export function GlobalErrorBanner() {
  const message = globalError.value;
  if (!message) return null;

  return (
    <div class={styles.wrap}>
      <Banner
        tone="bad"
        testId="banner-global-error"
        title={t("app.banner.globalErrorTitle")}
        detail={message}
        actions={<CopyButton text={message} />}
        onDismiss={() => {
          globalError.value = null;
        }}
      />
    </div>
  );
}

/** «Kopier» — samme etikett som loggraden under Avansert; ingen ny nøkkel
 *  for noe katalogen allerede har et ord for. */
function CopyButton({ text }: { text: string }) {
  async function copy(): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Utklippstavlen kan være utilgjengelig (rettigheter, headless rigg).
      // Banneret står uansett, med teksten synlig for hånd-avskrift — det er
      // derfor `detail` er den rå meldingen og ikke bare en kode.
    }
  }
  return (
    <Button
      variant="ghost"
      testId="banner-global-error-copy"
      onClick={() => void copy()}
    >
      {t("app.setup.advanced.copy")}
    </Button>
  );
}
