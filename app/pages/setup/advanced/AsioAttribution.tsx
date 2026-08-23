/**
 * Steinberg ASIO-attribusjonen.
 *
 * ## Hvorfor dette er en komponent og ikke en kommentar
 *
 * ASIO SDK-en er Steinbergs, gratis å bruke og LISENSIERT MOT ATTRIBUSJON:
 * varemerkenotisen må vises i produktet. Windows-utgivelsen bygges med
 * `--features …,asio,…` (`release.yml`), så den utgivelsen skylder notisen.
 *
 * Den bodde i `legacy/renderer/index.html` som et kort på System-fanen, avslørt
 * av `general-page.ts` når UA-en sa Windows. Fase B slettet den fila, og
 * notisen fulgte med — som er nøyaktig den typen tap som ikke merkes, fordi
 * ingen test og ingen bruker savner en setning de aldri leste. Den er bygget
 * opp igjen her, på Avansert, som den nærmeste flaten som finnes.
 *
 * ## Bare på Windows
 *
 * Samme regel som før: kortet vises der ASIO finnes. Avgjørelsen kommer fra
 * `detectOs` — den samme kilden som bestemmer om DirectShow-valget finnes i
 * `EngineRow` — og ikke fra en delstreng i UA-linja, fordi «Winamp» inneholder
 * «win».
 *
 * ## Notisen er ikke oversatt, og er derfor en modulkonstant
 *
 * Ordlyden er lisensteksten, ordrett. Å legge den i katalogen ville invitert
 * sju oversettelser av en juridisk formulering som skal stå på engelsk. Huset
 * har allerede formen for dette (`SundayRec`, `dBFS`): en modulkonstant, ikke
 * JSX-tekst. Prosagaten kan ikke vite at noe ikke skal oversettes, og skal
 * ikke måtte gjette.
 */

import { t } from "../../../i18n";
import { Card } from "../../../ui/Card/Card";
import { currentOs, type Os } from "./platform-core";

/** Lisensteksten, ordrett. Se `docs/BUILD_ASIO.md`. */
const ASIO_NOTICE =
  "ASIO Driver Interface Technology by Steinberg Media Technologies GmbH. " +
  "ASIO is a trademark and software of Steinberg Media Technologies GmbH.";

/** `os` er injisert så BEGGE grenene kan drives i node-gaten. Uten den kunne
 *  testen bare bevise at kortet er borte på maskinen den kjører på, som er den
 *  ene halvdelen som ikke er en lisensforpliktelse. */
export function AsioAttribution({ os = currentOs() }: { os?: Os } = {}) {
  if (os !== "win") return null;
  return (
    <Card
      title={t("app.setup.advanced.audioTech")}
      description={ASIO_NOTICE}
      testId="advanced-asio-attribution"
    />
  );
}
