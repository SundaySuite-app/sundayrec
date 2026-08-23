/**
 * Oppdateringssjekken — timeren P1b lot være, og den ene lytteren på løpet.
 *
 * ## Hvorfor den MÅ finnes
 *
 * P1b skrev det ned som en konsekvens å ta før skallet byttes: uten denne
 * modulen **sjekker det nye skallet aldri etter oppdateringer av seg selv**.
 * Det er ikke bare en manglende bekvemmelighet — det er den samme veien
 * beta-ringens kill-switch når fram til folk. `docs/ROLLBACK.md` sier det rett
 * ut: å pause en kanal på Workeren rekker en allerede kjørende installasjon
 * innen timen, nettopp fordi appen spør omtrent hver time. Et skall som ikke
 * spør, er et skall ingen kan stoppe en dårlig utgivelse fra.
 *
 * Kill-switchen har ingen klient-side bryter å respektere: den virker ved at
 * feeden slutter å tilby en versjon. Det eneste klienten skylder den, er å
 * spørre.
 *
 * ## PRIVACY.md er kontrakten
 *
 * «Slår du den av, tar appen ikke kontakt med serveren — verken ved oppstart
 * eller den vanlige sjekken hver time.» Det er et løfte om appen som KJØRER,
 * ikke om neste oppstart, så avgjørelsen tas på nytt hver gang innstillingen
 * endrer seg — ikke én gang ved oppkobling. Det var nøyaktig feilen i
 * revisjonsfunn #11: gaten ble lest FØR de lagrede innstillingene hadde landet,
 * så `undefined !== false` armet planen på hver eneste oppstart uansett hva
 * eieren hadde valgt.
 *
 * Her kan den feilen ikke gjenoppstå på samme måte: `initAutoUpdate()` kalles
 * ETTER `hydrateSettings()` i `app/main.tsx`, og effekten leser signalet, som
 * per definisjon er det som står lagret.
 *
 * Den manuelle knappen («Se etter oppdateringer nå») er med vilje UTENFOR
 * denne modulens rekkevidde — PRIVACY.md har den som sitt ene unntak, fordi et
 * trykk der er eieren som spør.
 *
 * ## Hva sjekken faktisk gjør
 *
 * `update_check` SPØR, og ikke noe mer. Nedlastingen skjer i
 * `update_download_install`, som bare kjøres når noen trykker på knappen. Så
 * raden under Avansert sier «Sjekker hver time», ikke «laster ned i
 * bakgrunnen» — det siste ville vært en påstand om nettbruk som ikke skjer.
 *
 * ## Én lytter, ikke to
 *
 * De sju `update-*`-kanalene shimmen syntetiserer abonneres her, ÉN gang, og
 * fasen bor i et signal. `UpdateRow` leser det signalet i stedet for å
 * abonnere selv: to lyttere på det samme løpet som hver holder sin egen
 * tilstand er skjøtefeilen `reference-seam-bugs` handler om, og her ville den
 * betydd at raden og banneret kunne si to forskjellige ting om den samme
 * nedlastingen.
 */

import { effect, signal } from "@preact/signals";

import {
  AUTO_UPDATE_INTERVAL_MS,
  autoUpdateEnabled,
  planAutoUpdateSchedule,
} from "@lib/pages/auto-update-schedule-core";

import { dismissBanner, raiseBanner } from "./banners";
import { settings } from "./settings";
import {
  phaseFromEvent,
  UPDATE_CHANNELS,
  type UpdatePhase,
} from "./update-core";

/** Hvor i oppdateringsløpet vi er. `idle` = ingen har spurt ennå. */
export const updatePhase = signal<UpdatePhase>({ kind: "idle" });

/** Den armede timeren, eller `null`. På modulnivå fordi den overlever kallet
 *  som armet den: bryteren må kunne kansellere en timer noen andre startet. */
let timer: ReturnType<typeof setInterval> | null = null;

/**
 * Få verden til å stemme med innstillingen, begge veier.
 *
 * Trygg å kalle om og om igjen: `planAutoUpdateSchedule` rapporterer bare
 * OVERGANGER, så å re-arme noe som allerede er armet er en no-op i stedet for
 * en andre timer. To timere er dobbelt så mye trafikk som eieren fikk vite om,
 * og ingenting på skjermen ville noensinne vist det.
 */
export function applyAutoUpdateSchedule(): void {
  const action = planAutoUpdateSchedule(
    timer !== null,
    autoUpdateEnabled(settings.value.autoUpdate),
  );
  if (action.stop && timer !== null) {
    clearInterval(timer);
    timer = null;
  }
  if (action.start) {
    // Å arme er «sjekk nå, og så hver time». Oppstart og et bytte midt i økta
    // tar dermed samme vei, i stedet for å være to adferder å holde i takt.
    void window.api.checkForUpdates();
    timer = setInterval(() => {
      void window.api.checkForUpdates();
    }, AUTO_UPDATE_INTERVAL_MS);
  }
}

/**
 * Fasen → bannerkøen.
 *
 * Bare de tre som er verdt å avbryte for. En sjekk som PÅGÅR, et «du er
 * oppdatert» eller en sjekk som feilet er ikke noe en frivillig fem minutter
 * før gudstjenesten skal se en gul stripe om — de står i raden under Avansert,
 * der noen har spurt. Et banner for hver fase ville lært folk å lukke bannere
 * uten å lese dem.
 *
 * `restarting` rører ikke køen: det som står, står, mens appen forsøker å
 * starte på nytt. Å fjerne banneret i det øyeblikket ville tatt bort den ene
 * tilbakemeldingen på at knappen gjorde noe.
 */
function syncUpdateBanner(phase: UpdatePhase): void {
  switch (phase.kind) {
    case "available":
      raiseBanner({
        key: "update",
        state: "available",
        version: phase.version,
        percent: 0,
      });
      return;
    case "downloading":
      raiseBanner({
        key: "update",
        state: "downloading",
        version: "",
        percent: phase.percent,
      });
      return;
    case "ready":
      raiseBanner({
        key: "update",
        state: "ready",
        version: phase.version,
        percent: 100,
      });
      return;
    case "restarting":
      return;
    default:
      dismissBanner("update");
  }
}

let dispose: (() => void) | null = null;

/**
 * Koble timeren og lytterne. Idempotent — et andre kall gir den samme
 * opprydderen.
 *
 * Effekten leser `settings.value`, altså HELE innstillingssignalet, så den
 * kjører på nytt ved enhver lagring. Det er med vilje og gratis: planen
 * rapporterer bare overganger, så alt annet enn et faktisk bytte av
 * «Oppdater automatisk» er en no-op.
 */
export function initAutoUpdate(): () => void {
  if (dispose) return dispose;

  const offs = UPDATE_CHANNELS.map((channel) =>
    window.api.on(channel, (payload: unknown) => {
      const next = phaseFromEvent(channel, payload);
      // En ukjent kanal ignoreres i stedet for å kaste inne i en
      // event-callback der ingenting fanger det.
      if (next) updatePhase.value = next;
    }),
  );

  const stopBanner = effect(() => syncUpdateBanner(updatePhase.value));
  const stopSchedule = effect(() => {
    void settings.value;
    applyAutoUpdateSchedule();
  });

  dispose = () => {
    for (const off of offs) off?.();
    stopBanner();
    stopSchedule();
    if (timer !== null) {
      clearInterval(timer);
      timer = null;
    }
    dispose = null;
  };
  return dispose;
}
