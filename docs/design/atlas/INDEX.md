# Atlas — SundayRec slik appen ER (Fase A)

> ⚠️ **Atlaset fotograferer v0.15-skallet — appen ser ikke slik ut lenger.**
> Fase B byttet ut hele skallet, og **D2** (v0.16.0-beta.2) flyttet det igjen:
> skinnen har to destinasjoner og et tannhjul nederst (ikke tre faner), OPPTAK
> er et kontrollrom der de fem spørsmålene folder seg ut på stedet, og
> Innstillinger er kirkeprofilen + Avansert. Scenene under (`home--*`,
> `settings-audio--*`, `schedule--*`) er sidene som ble erstattet.
>
> Bildene består likevel: de er Fase A-fasiten — «slik var det» — og de er
> begrunnelsen bak halvparten av beslutningene i redesignet. **Re-fotografering
> er en restanse**, og den er en NY scenetabell, ikke en re-peking av denne
> (fotografen `e2e/atlas/**` er slettet; se `../../APP-SHELL.md` §«Etter byttet»
> punkt 4). Hvordan D2-skallet ser ut i dag er tegnet i
> [../canvas/FASE-D2-KONTROLLROM.html](../canvas/FASE-D2-KONTROLLROM.html).

Hvert bilde er ett skjermbilde av appen som den står på `main` etter Fase R
(PR #139 + #141). Ingenting her er et forslag; dette er dokumentasjon av
nåtilstanden, og inndata til Fase D (redesign). IA-uttrekket ligger i
[../ATLAS.md](../ATLAS.md).

**Vindu:** 1180×760 (Tauri-vinduets standardstørrelse). Utvalgte hovedsider er i
tillegg fotografert på 960×640 — appens minimumsvindu — med suffikset
`--960x640`. Sider som ruller forbi vindushøyden har et `--full`-skudd som
viser hele siden (vindushøyden settes til innholdets høyde; Playwrights egen
`fullPage` gir ingenting her, fordi appen ruller inne i `#main`, ikke i
dokumentet).

**Språk:** `no/` og `en/`. De fem andre språkene i språkvelgeren er satt på
pause og er ikke fotografert.

**Total størrelse:** 8.8 MB i 181 PNG-er. Komprimert med `magick mogrify -colors 256`: 18.5 MB → 8.8 MB.

## Kjøre på nytt

```bash
npm run atlas                 # hele atlaset (starter Vite selv, port 1420)
npm run atlas -- -g "editor"  # bare scener som matcher
```

Atlaset er bevisst utenfor `npm run check` og utenfor CI: det er et
fotoapparat, ikke en port. `playwright.config.ts` ignorerer `e2e/atlas/`.

## Scener

| Scene-id | Side | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- | --- |
| `home--kald-forstegangs` | Hjem | Kald app: ingen enhet valgt, ingen lagringsmappe, ingen tidsplan | `COLD` | `no/home--kald-forstegangs.png` | `en/home--kald-forstegangs.png` | `no/home--kald-forstegangs--960x640.png` |
| `home--klar-med-enhet` | Hjem | Klar: mikser koblet til, neste gudstjeneste planlagt, opptak i historikken | `LIVE` | `no/home--klar-med-enhet.png` | `en/home--klar-med-enhet.png` | `no/home--klar-med-enhet--full.png` · `en/home--klar-med-enhet--full.png` |
| `home--nivaa-live` | Hjem | Lydnivå live — VU-målerne får ekte pakker fra vu://levels | `vu://levels` | `no/home--nivaa-live.png` | `en/home--nivaa-live.png` | — |
| `home--enhet-borte` | Hjem | Lagret mikser finnes ikke lenger — hero-advarsel «Koble til …» | `list_audio_devices:[]` | `no/home--enhet-borte.png` | `en/home--enhet-borte.png` | — |
| `home--lite-diskplass` | Hjem | 0,6 GB ledig — lagringskortet blir rødt | `get_disk_space` | `no/home--lite-diskplass.png` | `en/home--lite-diskplass.png` | — |
| `home--forhandssjekk` | Hjem | Pre-start-sjekken fant feil og advarsel (30 min før start) | `scheduler://preflight` | `no/home--forhandssjekk.png` | `en/home--forhandssjekk.png` | `no/home--forhandssjekk--full.png` · `en/home--forhandssjekk--full.png` |
| `home--tapt-opptak` | Hjem | Et planlagt opptak ble aldri tatt — kort + rød banner | `scheduler://missed` | `no/home--tapt-opptak.png` | `en/home--tapt-opptak.png` | `no/home--tapt-opptak--full.png` · `en/home--tapt-opptak--full.png` |
| `home--backend-feil` | Hjem | Terminal opptaksfeil — global feilstripe øverst | `recording://error` | `no/home--backend-feil.png` | `en/home--backend-feil.png` | — |
| `home--kvalitetsalarm` | Hjem | Fila mangler lyd — datatap-banner med «Vis opptak» | `recording://quality` | `no/home--kvalitetsalarm.png` | `en/home--kvalitetsalarm.png` | — |
| `home--samtykkekort` | Hjem | Engangsspørsmålet om diagnostikk (needsPrompt) | `telemetry_consent_get` | `no/home--samtykkekort.png` | `en/home--samtykkekort.png` | — |
| `home--video-pa` | Hjem | Video slått på — kamerastripe og forhåndsvisning | `videoEnabled` | `no/home--video-pa.png` | `en/home--video-pa.png` | `no/home--video-pa--full.png` · `en/home--video-pa--full.png` |
| `home--start-dialog` | Hjem | «Start opptak nå»-dialogen | `modal-manual` | `no/home--start-dialog.png` | `en/home--start-dialog.png` | — |
| `home--start-dialog-video` | Hjem | «Start opptak nå» med video slått på — kameravalg i dialogen | `modal-manual+video` | `no/home--start-dialog-video.png` | `en/home--start-dialog-video.png` | — |
| `opptak--pagar` | Opptaksoverlegg | Opptak pågår, nivåer fra recording://levels | `start_recording` | `no/opptak--pagar.png` | `en/opptak--pagar.png` | — |
| `opptak--avbrudd` | Opptaksoverlegg | Enheten falt ut — gjenkoblingsbanner + stillhetsvarsel | `recording://reconnecting` | `no/opptak--avbrudd.png` | `en/opptak--avbrudd.png` | — |
| `opptak--stopp-bekreftelse` | Opptaksoverlegg | «Stopp opptak?»-dialogen (protectRecording er på som standard) | `modal-confirm-stop` | `no/opptak--stopp-bekreftelse.png` | `en/opptak--stopp-bekreftelse.png` | — |
| `opptak--fullforer` | Opptaksoverlegg | Etter bekreftet stopp: «Fullfører opptak …», knappen låst | `stop_recording` | `no/opptak--fullforer.png` | `en/opptak--fullforer.png` | — |
| `schedule--tom` | Tidsplan | Ingen faste tider, ingen enkeltopptak | `slots:[]` | `no/schedule--tom.png` | `en/schedule--tom.png` | `no/schedule--tom--full.png` · `en/schedule--tom--full.png` · `no/schedule--tom--960x640.png` |
| `schedule--med-tider` | Tidsplan | Fast søndagstid + ett datert enkeltopptak | `slots+specials` | `no/schedule--med-tider.png` | `en/schedule--med-tider.png` | `no/schedule--med-tider--full.png` · `en/schedule--med-tider--full.png` |
| `schedule--tid-editor` | Tidsplan | Redigering av en fast tid (dagvelger, klokkeslett, maks lengde) | `#btn-add-slot` | `no/schedule--tid-editor.png` | `en/schedule--tid-editor.png` | `no/schedule--tid-editor--full.png` · `en/schedule--tid-editor--full.png` |
| `schedule--vekking-avansert` | Tidsplan | «Avansert» utvidet: vekking fra dvale, strøm, søvnkonfig, test | `#btn-adv-toggle` | `no/schedule--vekking-avansert.png` | `en/schedule--vekking-avansert.png` | `no/schedule--vekking-avansert--full.png` · `en/schedule--vekking-avansert--full.png` |
| `schedule--dagsdetalj` | Tidsplan | En kalenderdag valgt — hva som skjer den dagen | `kalenderklikk` | `no/schedule--dagsdetalj.png` | `en/schedule--dagsdetalj.png` | — |
| `settings-audio--ingen-enheter` | Innstillinger › Lyd | Ingen lydenheter funnet | `list_audio_devices:[]` | `no/settings-audio--ingen-enheter.png` | `en/settings-audio--ingen-enheter.png` | `no/settings-audio--ingen-enheter--full.png` · `en/settings-audio--ingen-enheter--full.png` · `no/settings-audio--ingen-enheter--960x640.png` |
| `settings-audio--enheter` | Innstillinger › Lyd | To enheter, mikseren valgt (32 kanaler) | `LIVE` | `no/settings-audio--enheter.png` | `en/settings-audio--enheter.png` | `no/settings-audio--enheter--full.png` · `en/settings-audio--enheter--full.png` |
| `settings-audio--kanalrutenett` | Innstillinger › Lyd | Kanalrutenettet for en 32-kanals mikser, med lagret L/R | `deviceChannels` | `no/settings-audio--kanalrutenett.png` | `en/settings-audio--kanalrutenett.png` | `no/settings-audio--kanalrutenett--full.png` · `en/settings-audio--kanalrutenett--full.png` |
| `settings-audio--diagnose` | Innstillinger › Lyd | Lydenhetsdiagnosen (modal) — rader + full systemrapport | `diagnose_audio` | `no/settings-audio--diagnose.png` | `en/settings-audio--diagnose.png` | — |
| `settings-video--av` | Innstillinger › Video | Video slått av — alt annet skjult | `videoEnabled:false` | `no/settings-video--av.png` | `en/settings-video--av.png` | `no/settings-video--av--960x640.png` |
| `settings-video--pa` | Innstillinger › Video | Video slått på — kameravalg og «behold separat lydfil» | `videoEnabled:true` | `no/settings-video--pa.png` | `en/settings-video--pa.png` | — |
| `settings-files--standard` | Innstillinger › Opptak | Mappe, filnavn, format, opprydding, stopp ved stillhet, pre-roll | `READY` | `no/settings-files--standard.png` | `en/settings-files--standard.png` | `no/settings-files--standard--full.png` · `en/settings-files--standard--full.png` · `no/settings-files--standard--960x640.png` |
| `settings-files--stillhet-pa` | Innstillinger › Opptak | «Stopp ved stillhet» på — terskel i dBFS og tidsavbrudd synlig | `stopOnSilence` | `no/settings-files--stillhet-pa.png` | `en/settings-files--stillhet-pa.png` | `no/settings-files--stillhet-pa--full.png` · `en/settings-files--stillhet-pa--full.png` |
| `settings-sharing--standard` | Innstillinger › Deling | Varsler + e-post ved feil (alt som er igjen etter Fase R) | `READY` | `no/settings-sharing--standard.png` | `en/settings-sharing--standard.png` | `no/settings-sharing--standard--960x640.png` |
| `settings-sharing--smtp` | Innstillinger › Deling | E-postvarsel på, SMTP-feltene åpne | `emailOnError` | `no/settings-sharing--smtp.png` | `en/settings-sharing--smtp.png` | `no/settings-sharing--smtp--full.png` · `en/settings-sharing--smtp--full.png` |
| `settings-general--standard` | Innstillinger › System | Språk, kirkeprofil, system, oppdatering, logg, diagnostikk | `READY` | `no/settings-general--standard.png` | `en/settings-general--standard.png` | `no/settings-general--standard--full.png` · `en/settings-general--standard--full.png` · `no/settings-general--standard--960x640.png` |
| `settings-general--telemetri-preview` | Innstillinger › System | «Vis hva som sendes» — hele nyttelasten som JSON | `telemetry_preview_payload` | `no/settings-general--telemetri-preview.png` | `en/settings-general--telemetri-preview.png` | — |
| `settings-general--oppdatering-tilgjengelig` | Innstillinger › System | Oppdateringskortet: en ny versjon finnes | `update_check:available` | `no/settings-general--oppdatering-tilgjengelig.png` | `en/settings-general--oppdatering-tilgjengelig.png` | — |
| `settings-general--oppdatering-klar` | Innstillinger › System | Oppdateringskortet: nedlastet, klar til å installeres | `update_check:downloaded` | `no/settings-general--oppdatering-klar.png` | `en/settings-general--oppdatering-klar.png` | — |
| `settings-general--oppdatering-feil` | Innstillinger › System | Oppdateringskortet: sjekken feilet | `update_check:throws` | `no/settings-general--oppdatering-feil.png` | `en/settings-general--oppdatering-feil.png` | — |
| `settings-general--oppdatering-varsel` | Innstillinger › System | Oppdateringsvarselet i sidepanelet (update-toast) | `update-toast` | `no/settings-general--oppdatering-varsel.png` | `en/settings-general--oppdatering-varsel.png` | — |
| `search--tom` | Historikk | Ingen opptak ennå | `recordings_list:[]` | `no/search--tom.png` | `en/search--tom.png` | `no/search--tom--960x640.png` |
| `search--med-opptak` | Historikk | Fem opptak: langt, kort, video, med notat, langt filnavn | `recordings_list` | `no/search--med-opptak.png` | `en/search--med-opptak.png` | — |
| `search--treff` | Historikk | Søk på «bønne» — filtrert liste, statistikken følger filteret | `#search-query` | `no/search--treff.png` | `en/search--treff.png` | — |
| `search--ingen-treff` | Historikk | Søk uten treff — egen melding, ikke «ingen opptak ennå» | `#search-query` | `no/search--ingen-treff.png` | `en/search--ingen-treff.png` | — |
| `search--flere-verktoy` | Historikk | «Flere»-panelet: slett feilede, rydd historikk | `#btn-history-more` | `no/search--flere-verktoy.png` | `en/search--flere-verktoy.png` | — |
| `search--papirkurv-fylt` | Historikk › Papirkurv | Ett slettet opptak, med «tøm papirkurv» | `trash_list` | `no/search--papirkurv-fylt.png` | `en/search--papirkurv-fylt.png` | — |
| `search--notat-dialog` | Historikk | Notat-dialogen på en rad | `modal-note` | `no/search--notat-dialog.png` | `en/search--notat-dialog.png` | — |
| `search--slett-angre` | Historikk | Sletting: ingen bekreftelse, men en «Angre»-toast (suksess-toast) | `trash_move` | `no/search--slett-angre.png` | `en/search--slett-angre.png` | — |
| `editor--tom` | Rediger | Ingen fil åpen — slippsone og siste opptak | `goto=editor` | `no/editor--tom.png` | `en/editor--tom.png` | `no/editor--tom--full.png` · `en/editor--tom--full.png` · `no/editor--tom--960x640.png` |
| `editor--laster` | Rediger | «Analyserer …» — fila leses og bølgeformen bygges | `editor_load_recording:pending` | `no/editor--laster.png` | `en/editor--laster.png` | — |
| `editor--feil` | Rediger | Fila kunne ikke leses | `editor_load_recording:throws` | `no/editor--feil.png` | `en/editor--feil.png` | — |
| `editor--lyd-fane` | Rediger › Lyd | Åpnet opptak: bølgeform, normalisering, intro/outro, mastering | `editorFixtures` | `no/editor--lyd-fane.png` | `en/editor--lyd-fane.png` | `no/editor--lyd-fane--full.png` · `en/editor--lyd-fane--full.png` · `no/editor--lyd-fane--960x640.png` |
| `editor--innhold-fane` | Rediger › Innhold | Metadata: tittel, taler, beskrivelse | `editorFixtures` | `no/editor--innhold-fane.png` | `en/editor--innhold-fane.png` | — |
| `editor--klipp-fane` | Rediger › Klipp | Segmenter funnet, prekenvelger, «Marker preken automatisk» | `editor_segments` | `no/editor--klipp-fane.png` | `en/editor--klipp-fane.png` | `no/editor--klipp-fane--full.png` · `en/editor--klipp-fane--full.png` |
| `editor--kuttliste` | Rediger › Klipp | Etter «Marker preken automatisk»: to kutt i kuttlisten | `#btn-apply-auto-trim` | `no/editor--kuttliste.png` | `en/editor--kuttliste.png` | `no/editor--kuttliste--full.png` · `en/editor--kuttliste--full.png` |
| `editor--mastering-panel` | Rediger › Lyd | Mastering-panelet utvidet (ett av fem steder mastring finnes — ATLAS.md §3c) | `editor_master_presets` | `no/editor--mastering-panel.png` | `en/editor--mastering-panel.png` | `no/editor--mastering-panel--full.png` · `en/editor--mastering-panel--full.png` |
| `editor--eksport-modal` | Rediger | Eksportmodalen for lyd: format, bitrate, destinasjon, lydforbedring | `#btn-editor-save` | `no/editor--eksport-modal.png` | `en/editor--eksport-modal.png` | `no/editor--eksport-modal--full.png` · `en/editor--eksport-modal--full.png` |
| `editor--eksport-modal-video` | Rediger | Eksportmodalen for et videoopptak: eksporttype, kodek, format | `hasVideo:true` | `no/editor--eksport-modal-video.png` | `en/editor--eksport-modal-video.png` | `no/editor--eksport-modal-video--full.png` · `en/editor--eksport-modal-video--full.png` |
| `onboarding--1-velkommen` | Første oppstart | Steg 1 — velkommen | `onboardingDone:false` | `no/onboarding--1-velkommen.png` | `en/onboarding--1-velkommen.png` | `no/onboarding--1-velkommen--960x640.png` |
| `onboarding--2-lydenhet` | Første oppstart | Steg 2 — hvilken lydenhet bruker dere? | `onboardingDone:false` | `no/onboarding--2-lydenhet.png` | `en/onboarding--2-lydenhet.png` | — |
| `onboarding--3-lydtest` | Første oppstart | Steg 3 — test at lyden fungerer (lydtest-porten) | `onboardingDone:false` | `no/onboarding--3-lydtest.png` | `en/onboarding--3-lydtest.png` | — |
| `onboarding--4-tidsplan` | Første oppstart | Steg 4 — ukentlig automatisk opptak | `onboardingDone:false` | `no/onboarding--4-tidsplan.png` | `en/onboarding--4-tidsplan.png` | — |
| `onboarding--5-samtykke` | Første oppstart | Steg 5 — vil du hjelpe oss? (diagnostikk-samtykke) | `onboardingDone:false` | `no/onboarding--5-samtykke.png` | `en/onboarding--5-samtykke.png` | — |
| `onboarding--6-ferdig` | Første oppstart | Steg 6 — alt er klart | `onboardingDone:false` | `no/onboarding--6-ferdig.png` | `en/onboarding--6-ferdig.png` | — |
| `toast--lagring-feilet` | Innstillinger › System | Feil-toast: innstillingen kunne ikke lagres | `settings_save:throws` | `no/toast--lagring-feilet.png` | `en/toast--lagring-feilet.png` | — |

## Hvordan scenene lages

Scenetabellen ligger i `e2e/atlas/scenes.ts`. Hver scene er
`{ fixtures, settings, goto }` gjennom `e2e/harness.ts` (api-shim-sømmen),
pluss eventuelle klikk. `e2e/atlas/harness.ts` legger til to ting den vanlige
nettleser-tieren ikke har:

1. **Backend-hendelser.** Halvparten av tilstandene males av en Tauri-event, ikke
   av et kall: opptaksmåleren (`recording://levels`), tapt-opptak-kortet
   (`scheduler://missed`), forhåndssjekken (`scheduler://preflight`),
   gjenkoblingsbanneret, den globale feilstripa. Broen husker hvilken
   callback-id som abonnerte på hvilket eventnavn, og
   `window.__ATLAS_EMIT__(event, payload)` fyrer dem av. Fyrer den mot et
   eventnavn ingen lytter på, feiler scenen — den fotograferer ikke feil skjerm.
2. **En fast klokke.** `page.clock.setFixedTime` låser `Date.now()` til
   søndag 23. august 2026 kl. 10:55, slik at «om 3 dager», «for 2 timer siden»
   og opptakstelleren er de samme i to kjøringer.
