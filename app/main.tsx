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
 *      TO trær: skallet i `#app`, dialogen og toastene i `#overlays`. De er
 *      SØSKEN fordi DialogHost setter `inert` på `#app` mens den er åpen — en
 *      dialog inne i `#app` ville slått av seg selv.
 *   4. `installGlobalNavigation` FØR alt asynkront: `window.showPage` er
 *      kontrakten `e2e/harness.ts` venter på før et spec får gjøre noe som
 *      helst, og den api-shimmens egen `?goto=`-blokk poller på.
 *   5. `hydrateSettings` før `setLocale`, fordi språket ER en innstilling.
 *   6. Butikkene etter innstillingene, fordi de avleder fra dem — og
 *      `initAutoUpdate` er den ene der rekkefølgen er et LØFTE og ikke bare
 *      ryddighet: en oppdateringssjekk armet før den lagrede `autoUpdate` er
 *      lest, kontakter serveren for en eier som har sagt nei.
 *   7. `?goto=` etter alt det — en dyplenke skal lande på en app som er ferdig
 *      å våkne, ikke midt i det.
 *   8. Onboarding-porten sist, og BARE når det ikke var en dyplenke.
 *
 * Punkt 7 gjøres her selv om api-shimmen også har en `?goto=`-blokk. Den
 * poller `window.showPage` med 150 ms forsinkelse — den er laget for
 * skjermbilde-passene. Å gjøre det selv betyr at første frame allerede er
 * riktig side, i stedet for at brukeren ser TA OPP blinke forbi på vei til
 * OPPSETT.
 *
 * ⚠️ De to lander på samme rute, men gjentakelsen er bare idempotent så lenge
 * INGENTING navigerer i mellomtiden. Gjør noe det — et klikk i de 150 ms-ene,
 * eller et e2e-spec som er raskere enn en frivillig — river shimmens
 * gjentakelse skjermen tilbake til dyplenken, og brukeren står et sted hun
 * ikke valgte. Derfor får shimmen en `navigate` som DROPPER nøyaktig den
 * gjentakelsen (`isDeepLinkRepeat` under) og ingenting annet. Slottet finnes
 * for akkurat denne typen: en vert som vet noe shimmen ikke kan vite.
 */

import "./styles/base.css";

import { setShimNotifier } from "@lib/api-shim";
import { parseGoto } from "@lib/goto-core";
import { render } from "preact";

import { installEditorEntry } from "./editor/entry";
import { resolveStartupLocale, setLocale, t } from "./i18n";
import {
  installGlobalNavigation,
  installTrayNavigation,
  navigate,
  type NavigateOpts,
} from "./router/router";
import { Shell, Overlays } from "./Shell";
import { loadAppVersion } from "./state/app-info";
import { currentOs, platformClass } from "./state/platform-core";
import { initAutoUpdate } from "./state/auto-update";
import { initBackendWarnings } from "./state/backend-warning";
import { initDisk } from "./state/disk";
import { installErrorHandlers } from "./state/global-error";
import { initNextRecording } from "./state/next-recording";
import { initPreroll } from "./state/preroll";
import { initRecording } from "./state/recording";
import { loadRecordingCount } from "./state/recordings";
import { initRetention } from "./state/retention";
import { hydrateSettings, settings } from "./state/settings";
import { toast } from "./ui/toast";

/** Dyplenken denne oppstarten kom med, eller `null`. */
const deepLink = parseGoto(location.search);
/** Har den allerede landet? (Av oss, eller av shimmens egen blokk.) */
let deepLinkApplied = false;
/**
 * Hvor mange gjentakelser vi har lov til å slippe. ÉN, fordi shimmen gjentar
 * nøyaktig én gang (`setTimeout(tryGoto, 150)`). En tidsbasert grense ville
 * vært et tall å gjette; en engangsbillett er den samme grensen uten gjetting,
 * og en senere `showPage("setup")` fra menylinjen kommer alltid fram.
 */
let repeatsToDrop = deepLink ? 1 : 0;

/** Er dette PRESIS dyplenken — samme side, samme fane? */
function matchesDeepLink(page: string, opts?: NavigateOpts): boolean {
  if (!deepLink || page !== deepLink.page) return false;
  return (opts?.tab ?? undefined) === (deepLink.tab ?? undefined);
}

/**
 * `navigate` for shimmen — og for `window.showPage`, som shimmen bruker når
 * dyplenken ikke har en fane. Alt slippes gjennom, unntatt ÉN gjentakelse av en
 * dyplenke som allerede har landet. Se toppen av fila.
 */
function navigateFromShim(page: string, opts: NavigateOpts = {}): void {
  if (deepLinkApplied && repeatsToDrop > 0 && matchesDeepLink(page, opts)) {
    repeatsToDrop -= 1;
    return;
  }
  navigate(page, opts);
  if (matchesDeepLink(page, opts)) deepLinkApplied = true;
}

// 2. Skallets egne flater inn i shimmen, før noe kan feile.
setShimNotifier({ toast, navigate: navigateFromShim, t });

const host = document.getElementById("app");
if (!host) {
  // En hvit skjerm med en konsollfeil er feilmodusen dette skal fange høylytt
  // i stedet for stille.
  throw new Error('app/index.html mangler sitt <div id="app">-monteringspunkt');
}

const overlayHost = document.getElementById("overlays");
if (!overlayHost) {
  // Uten dette ville en bekreftelsesdialog aldri blitt vist, og
  // `confirmDialog()` ville hengt for alltid — altså en app der en farlig
  // handling stille ikke skjer. Høylytt er den eneste riktige feilmodusen.
  throw new Error(
    'app/index.html mangler sitt <div id="overlays">-monteringspunkt',
  );
}

/*
 * Plattformen som en KLASSE på rota, og den må stå FØR `render` — ikke fordi
 * noe er avhengig av den, men fordi den er en målregel: på macOS gir
 * `.platform-darwin` topplinja venstremargen som holder logoen klar av
 * trafikklysene (`PageShell.module.css`). Settes den etterpå, males første
 * frame med feil marg og logoen hopper 84 piksler idet appen åpner — en
 * feil som ser ut som at appen ikke er ferdig lastet.
 *
 * Én klasse, ikke `platform-darwin platform-not-win`: CSS spør «er dette
 * macOS?», og et negativt navn er en påstand til som kan bli feil alene.
 */
document.documentElement.classList.add(platformClass(currentOs()));

// TODO(S1b): `?probe=` forsvinner sammen med `app/dev/`.
const probe = new URLSearchParams(location.search).get("probe");

// 3.
render(<Shell probe={probe} />, host);
render(<Overlays />, overlayHost);

// 4. Kontraktene tray, dyplenker og harness.ts hviler på.
installGlobalNavigation((id) => navigateFromShim(id));
installTrayNavigation();
installErrorHandlers();
// `window.openEditorWithFile` — samme kontrakt som legacy-skallet, og
// `e2e/editor.spec.ts` + atlas-scenene åpner editoren gjennom den. Her, ved
// siden av `showPage`, fordi den hører til samme klasse: noe UTENFOR treet
// hviler på at den finnes.
installEditorEntry();

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
  initDisk();
  // ETTER de tre over, og det er ikke tilfeldig: `backend://warning` sin
  // dedupliseringsregel leser disken og enhetslisten for å avgjøre om skallet
  // allerede sier det samme, og den skriver til pre-rollens brikke når
  // bufferen er død. En lytter som ble installert foran butikkene sine ville
  // vært den samme skjøten som feilen den lukker.
  initBackendWarnings();
  // ETTER innstillingene, og det er hele poenget: gaten «Oppdater automatisk»
  // må leses fra det som faktisk står lagret. Revisjonsfunn #11 var nøyaktig
  // det motsatte — planen ble armet før den lagrede blobben hadde landet, så
  // `undefined !== false` kontaktet serveren på hver oppstart uansett hva
  // eieren hadde valgt. PRIVACY.md er kontrakten; se `state/auto-update.ts`.
  initAutoUpdate();
  // ETTER innstillingene av samme grunn som oppdateringssjekken: passet skal
  // avgjøres av det som faktisk står lagret (backenden leser `autoDeleteDays`
  // selv, per pass). Flytter det noe, sier toasten fra — se `state/retention`.
  initRetention();
  // Begge er engangslesninger skallet og Bibliotek viser: versjonen i
  // bunnlinja, og om biblioteket faktisk er tomt. Ingen `await` — en side som
  // venter på et tall den kan vise «ikke lest ennå» for, venter uten grunn.
  void loadAppVersion();
  void loadRecordingCount();

  // 7.
  if (deepLink) {
    // Ingen puls: denne veien finnes for rene skjermbilder, og en glødende
    // ramme ville vært i halvparten av dem.
    // Rakk shimmen det først? Da ER gjentakelsen brukt opp, og billetten skal
    // ikke bli liggende og spise et senere, ekte `showPage` til samme sted.
    if (deepLinkApplied) repeatsToDrop = 0;
    navigate(deepLink.page, { tab: deepLink.tab, highlight: false });
    deepLinkApplied = true;
    return;
  }

  // 8. Første gang: OPPSETT er ikke et sted brukeren valgte å gå, det er
  // starten. (`?goto=` tvinger `onboardingDone` sann i shimmen, så en dyplenke
  // kommer aldri hit.)
  if (!settings.peek().onboardingDone) navigate("setup", { firstRun: true });
}
