/**
 * Skallets inngang — og oppstartsrekkefølgen, som er det eneste her som ikke
 * er utskiftbart.
 *
 * Hver linje under står der den står fordi noe annet er avhengig av at den
 * allerede har skjedd:
 *
 *   1. `@lib/api-shim` installerer `window.api` som en SIDEEFFEKT av importen.
 *      Alt annet i denne fila snakker med backenden gjennom den.
 *   2. `setShimNotifier` FØR første mulige feil. Shimmen har tre kroker inn i
 *      vertsskallet (toast, navigate, t); uten dette kallet ville en feilet
 *      kommando malt en toast inn i legacy-skallets elementtre, som ikke
 *      finnes her.
 *   3. `render` før `installGlobalNavigation`, så det finnes noe å navigere I.
 *   4. `installGlobalNavigation` FØR alt asynkront: `window.showPage` er
 *      kontrakten `e2e/harness.ts` venter på før et spec får gjøre noe som
 *      helst, og den api-shimmens egen `?goto=`-blokk poller på.
 *   5. `hydrateSettings` før `setLocale`, fordi språket ER en innstilling.
 *   6. Butikkene etter innstillingene, fordi de avleder fra dem.
 *   7. `?goto=` etter alt det — en dyplenke skal lande på en app som er ferdig
 *      å våkne, ikke midt i det.
 *   8. Onboarding-porten sist, og BARE når det ikke var en dyplenke.
 *
 * Punkt 7 gjøres her selv om api-shimmen også har en `?goto=`-blokk. Den
 * poller `window.showPage` med 150 ms forsinkelse — den er laget for
 * skjermbilde-passene. Å gjøre det selv betyr at første frame allerede er
 * riktig side, i stedet for at brukeren ser TA OPP blinke forbi på vei til
 * OPPSETT. De to lander på samme rute, så den ene er en idempotent gjentakelse
 * av den andre.
 */

import { setShimNotifier } from "@lib/api-shim";
import { parseGoto } from "@lib/goto-core";
import { render } from "preact";

import { resolveStartupLocale, setLocale, t } from "./i18n";
import {
  installGlobalNavigation,
  installTrayNavigation,
  navigate,
} from "./router/router";
import { Shell } from "./Shell";
import { installErrorHandlers } from "./state/global-error";
import { initNextRecording } from "./state/next-recording";
import { initPreroll } from "./state/preroll";
import { initRecording } from "./state/recording";
import { hydrateSettings, settings } from "./state/settings";
import { toast } from "./ui/toast";

// 2. Skallets egne flater inn i shimmen, før noe kan feile.
setShimNotifier({ toast, navigate, t });

const host = document.getElementById("app");
if (!host) {
  // En hvit skjerm med en konsollfeil er feilmodusen dette skal fange høylytt
  // i stedet for stille.
  throw new Error('app/index.html mangler sitt <div id="app">-monteringspunkt');
}

// TODO(S1b): `?probe=` forsvinner sammen med `app/dev/`.
const probe = new URLSearchParams(location.search).get("probe");

// 3.
render(<Shell probe={probe} />, host);

// 4. Kontraktene tray, dyplenker og harness.ts hviler på.
installGlobalNavigation();
installTrayNavigation();
installErrorHandlers();

void boot();

async function boot(): Promise<void> {
  // 5.
  await hydrateSettings();
  await setLocale(resolveStartupLocale(settings.peek().language));

  // 6. Idempotente — de returnerer en oppryddingsfunksjon vi ikke trenger her,
  // fordi skallet lever like lenge som vinduet.
  initRecording();
  initNextRecording();
  initPreroll();

  // 7.
  const target = parseGoto(location.search);
  if (target) {
    // Ingen puls: denne veien finnes for rene skjermbilder, og en glødende
    // ramme ville vært i halvparten av dem.
    navigate(target.page, { tab: target.tab, highlight: false });
    return;
  }

  // 8. Første gang: OPPSETT er ikke et sted brukeren valgte å gå, det er
  // starten. (`?goto=` tvinger `onboardingDone` sann i shimmen, så en dyplenke
  // kommer aldri hit.)
  if (!settings.peek().onboardingDone) navigate("setup", { firstRun: true });
}
