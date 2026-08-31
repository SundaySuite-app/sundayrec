# Atlas — SundayRec slik appen ER

Hvert bilde er ett skjermbilde av appen slik den står på `main` i dag: D3-skallet
(topplinje + bunnlinje, tre destinasjoner og et tannhjul), D2-kontrollrommet på
OPPTAK, og V1-runden (diagnoserad, hevede treffflater, rettede tekster).
Ingenting her er et forslag — dette er den visuelle regresjonsbasen.

Arkivet fra v0.15 ligger i [`../atlas-v015/`](../atlas-v015/INDEX.md), sammen
med IA-uttrekket i [`../ATLAS.md`](../ATLAS.md). De to viser appen slik den
VAR, og skal ikke forveksles med denne mappa.

**Vindu:** 1180×760 (Tauri-vinduets standardstørrelse). Hovedscenen på hver
destinasjon er i tillegg fotografert på 1000×760 — bredden der kontrollrommet
faller til ÉN kolonne — med suffikset `--1000x760` (norsk bare: om en layout
overlever én kolonne er ikke et språkspørsmål). Sider som ruller forbi vindushøyden har
et `--full`-skudd som viser hele siden (vindushøyden settes til innholdets
høyde; Playwrights egen `fullPage` gir ingenting her, fordi appen ruller inne i
`#main`, ikke i dokumentet).

**Språk:** `no/` og `en/`. De fem andre språkene i språkvelgeren er satt på
pause og er ikke fotografert.

**Total størrelse:** 7.5 MB i 198 PNG-er. Komprimert med `magick mogrify -colors 256`: 21.6 MB → 7.5 MB.

## Kjøre på nytt

```bash
npm run atlas                    # hele atlaset (starter Vite selv, port 1421)
npm run atlas -- -g "editor"     # bare scener som matcher
SUNDAYREC_ATLAS_PORT=1431 npm run atlas   # når 1421 er opptatt
```

Atlaset er bevisst utenfor `npm run check` og utenfor CI: det er et
fotoapparat, ikke en port. `playwright.config.ts` ignorerer `e2e/atlas/` med
en regex (`npx playwright test --list | grep -c atlas` skal gi 0).

**Egen port med vilje.** Nettleser-tieren bruker `SUNDAYREC_E2E_PORT` (1420);
atlaset bruker `SUNDAYREC_ATLAS_PORT` (1421), begge med `--strictPort`. Uten
det ville en fotografering startet mens `npm run e2e` kjører festet seg til
DEN serveren gjennom `reuseExistingServer` — og i en worktree fotografert et
annet utsjekk enn det man står i.

**To kjøringer skal gi identiske filer.** Klokka er låst, VU-pakkene er
konstante og animasjonene er ferdige før lukkeren går. Diff-er
`shasum`-summene etter to kjøringer: en fil som endrer seg uten at koden gjorde
det, er en scene som ikke står stille.

## Scener

### Opptak

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `opptak--kald` | Kald start — ingen lydkilde valgt ennå | ingen `deviceId`; Start er sperret og sier hvorfor | [`no/opptak--kald.png`](no/opptak--kald.png) | [`en/opptak--kald.png`](en/opptak--kald.png) | [`no/opptak--kald--1000x760.png`](no/opptak--kald--1000x760.png) |
| `opptak--klar` | Klar — kilde valgt, plass på disken, neste og siste opptak | `deviceId:x32` + `scheduler_status.next` + tre opptak i lista | [`no/opptak--klar.png`](no/opptak--klar.png) | [`en/opptak--klar.png`](en/opptak--klar.png) | [`no/opptak--klar--full.png`](no/opptak--klar--full.png) · [`en/opptak--klar--full.png`](en/opptak--klar--full.png) · [`no/opptak--klar--1000x760.png`](no/opptak--klar--1000x760.png) |
| `opptak--kilde-borte` | Kilden er valgt, men ikke til stede nå | `list_audio_devices` uten `x32`; Start er ÅPEN, med advarsel | [`no/opptak--kilde-borte.png`](no/opptak--kilde-borte.png) | [`en/opptak--kilde-borte.png`](en/opptak--kilde-borte.png) | — |
| `opptak--kort-lyd` | Kilde-kortet foldet ut — hele «Hvilken lyd?» på stedet | `?goto=settings:audio` → anker `sound` | [`no/opptak--kort-lyd.png`](no/opptak--kort-lyd.png) | [`en/opptak--kort-lyd.png`](en/opptak--kort-lyd.png) | [`no/opptak--kort-lyd--full.png`](no/opptak--kort-lyd--full.png) · [`en/opptak--kort-lyd--full.png`](en/opptak--kort-lyd--full.png) |
| `opptak--kort-mappe` | «Hvor skal opptakene?» foldet ut | `?goto=settings:files` → anker `folder` | [`no/opptak--kort-mappe.png`](no/opptak--kort-mappe.png) | [`en/opptak--kort-mappe.png`](en/opptak--kort-mappe.png) | — |
| `opptak--kort-kvalitet` | «Hvilken kvalitet?» foldet ut | klikk `control-quality-expand` (ingen gammel fane peker hit) | [`no/opptak--kort-kvalitet.png`](no/opptak--kort-kvalitet.png) | [`en/opptak--kort-kvalitet.png`](en/opptak--kort-kvalitet.png) | [`no/opptak--kort-kvalitet--full.png`](no/opptak--kort-kvalitet--full.png) · [`en/opptak--kort-kvalitet--full.png`](en/opptak--kort-kvalitet--full.png) |
| `opptak--kort-kamera` | «Ta med kamera» foldet ut, med kameravalget | `?goto=settings:video` + `videoEnabled:true` — kortet kan BARE foldes ut når tillegget er på | [`no/opptak--kort-kamera.png`](no/opptak--kort-kamera.png) | [`en/opptak--kort-kamera.png`](en/opptak--kort-kamera.png) | [`no/opptak--kort-kamera--full.png`](no/opptak--kort-kamera--full.png) · [`en/opptak--kort-kamera--full.png`](en/opptak--kort-kamera--full.png) |
| `opptak--kort-auto` | «Ta opp automatisk» foldet ut, med to faste tider | `?goto=schedule` → anker `auto`; to slots i innstillingene | [`no/opptak--kort-auto.png`](no/opptak--kort-auto.png) | [`en/opptak--kort-auto.png`](en/opptak--kort-auto.png) | [`no/opptak--kort-auto--full.png`](no/opptak--kort-auto--full.png) · [`en/opptak--kort-auto--full.png`](en/opptak--kort-auto--full.png) |
| `opptak--kort-varsling` | «Varsling» foldet ut | `?goto=settings:sharing` → anker `notify` | [`no/opptak--kort-varsling.png`](no/opptak--kort-varsling.png) | [`en/opptak--kort-varsling.png`](en/opptak--kort-varsling.png) | [`no/opptak--kort-varsling--full.png`](no/opptak--kort-varsling--full.png) · [`en/opptak--kort-varsling--full.png`](en/opptak--kort-varsling--full.png) |
| `opptak--kamera-live` | Kamerabildet står — fasen `live` | `videoEnabled:true` + stubbet `getUserMedia` (canvas-strøm) | [`no/opptak--kamera-live.png`](no/opptak--kamera-live.png) | [`en/opptak--kamera-live.png`](en/opptak--kamera-live.png) | [`no/opptak--kamera-live--full.png`](no/opptak--kamera-live--full.png) · [`en/opptak--kamera-live--full.png`](en/opptak--kamera-live--full.png) |
| `opptak--kamera-nektet` | Kamerabildet — fasen `denied` (OS-et sa nei) | stubbet `getUserMedia` som kaster `NotAllowedError` | [`no/opptak--kamera-nektet.png`](no/opptak--kamera-nektet.png) | [`en/opptak--kamera-nektet.png`](en/opptak--kamera-nektet.png) | — |
| `opptak--kamera-borte` | Kamerabildet — det lagrede kameraet finnes ikke | `videoDeviceName` som ikke er blant `enumerateDevices` | [`no/opptak--kamera-borte.png`](no/opptak--kamera-borte.png) | [`en/opptak--kamera-borte.png`](en/opptak--kamera-borte.png) | — |
| `opptak--banner-avbrutt` | Banner: opptaket ble avbrutt, med grunnen i klartekst | `emit('recording-error', { code: 'device_disconnected' })` | [`no/opptak--banner-avbrutt.png`](no/opptak--banner-avbrutt.png) | [`en/opptak--banner-avbrutt.png`](en/opptak--banner-avbrutt.png) | — |
| `opptak--banner-lite-plass` | Banner: under to timer igjen på disken | `get_disk_space.freeBytes = 200 MB` | [`no/opptak--banner-lite-plass.png`](no/opptak--banner-lite-plass.png) | [`en/opptak--banner-lite-plass.png`](en/opptak--banner-lite-plass.png) | — |
| `opptak--banner-gikk-glipp` | Banner: et planlagt opptak ble aldri tatt | `emitEvent('scheduler://missed', [{ at, label }])` | [`no/opptak--banner-gikk-glipp.png`](no/opptak--banner-gikk-glipp.png) | [`en/opptak--banner-gikk-glipp.png`](en/opptak--banner-gikk-glipp.png) | — |
| `opptak--banner-forhandssjekk` | Banner: forhåndssjekken fant noe å se på | `media_permissions.microphone = denied` + ett `run_preflight`-funn | [`no/opptak--banner-forhandssjekk.png`](no/opptak--banner-forhandssjekk.png) | [`en/opptak--banner-forhandssjekk.png`](en/opptak--banner-forhandssjekk.png) | — |
| `opptak--samtykkekort` | Samtykkekortet — det ene spørsmålet om diagnosedata | `telemetry_consent_get.needsPrompt = true` | [`no/opptak--samtykkekort.png`](no/opptak--samtykkekort.png) | [`en/opptak--samtykkekort.png`](en/opptak--samtykkekort.png) | — |
| `opptak--kvittering` | Kvitteringen etter et ferdig opptak | `emit('recording-finished', { path })` | [`no/opptak--kvittering.png`](no/opptak--kvittering.png) | [`en/opptak--kvittering.png`](en/opptak--kvittering.png) | — |

### Opptaksoverlegget

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `overlegg--pagar` | Det tas opp — klokke, måler, kamerabilde og fakta | `emit('recording-overlay-stop', {state:'recording'})`, klokka +42 min, ekte JPEG-frame | [`no/overlegg--pagar.png`](no/overlegg--pagar.png) | [`en/overlegg--pagar.png`](en/overlegg--pagar.png) | — |
| `overlegg--stopp-dialog` | Stopp-bekreftelsen — «Fortsett å ta opp» er primærvalget | overlegget oppe, så klikk `overlay-stop` | [`no/overlegg--stopp-dialog.png`](no/overlegg--stopp-dialog.png) | [`en/overlegg--stopp-dialog.png`](en/overlegg--stopp-dialog.png) | — |
| `overlegg--nedtelling` | Auto-stopp om 15 minutter, med «+ 15 min» og «Avbryt» | `scheduled_stop_ms` = klokka + 57 min, så klokka flyttes til +42 | [`no/overlegg--nedtelling.png`](no/overlegg--nedtelling.png) | [`en/overlegg--nedtelling.png`](en/overlegg--nedtelling.png) | — |

### Redigering

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `redigering--bibliotek` | Biblioteket med opptak — dato, lengde, notat | tre rader i `recordings_list`, én med notat | [`no/redigering--bibliotek.png`](no/redigering--bibliotek.png) | [`en/redigering--bibliotek.png`](en/redigering--bibliotek.png) | [`no/redigering--bibliotek--1000x760.png`](no/redigering--bibliotek--1000x760.png) |
| `redigering--bibliotek-tomt` | Biblioteket er tomt — og sier hva man gjør nå | `recordings_list: []` | [`no/redigering--bibliotek-tomt.png`](no/redigering--bibliotek-tomt.png) | [`en/redigering--bibliotek-tomt.png`](en/redigering--bibliotek-tomt.png) | — |
| `redigering--sok-ingen-treff` | Søket ga ingen treff | fyll `library-search` med «finnesikke» | [`no/redigering--sok-ingen-treff.png`](no/redigering--sok-ingen-treff.png) | [`en/redigering--sok-ingen-treff.png`](en/redigering--sok-ingen-treff.png) | — |
| `redigering--papirkurv` | Papirkurven — med «slettes om N dager» | to rader i `trash_list`, så klikk `library-trash-open` (ikke nåbar via `?goto=`) | [`no/redigering--papirkurv.png`](no/redigering--papirkurv.png) | [`en/redigering--papirkurv.png`](en/redigering--papirkurv.png) | [`no/redigering--papirkurv--full.png`](no/redigering--papirkurv--full.png) · [`en/redigering--papirkurv--full.png`](en/redigering--papirkurv--full.png) |
| `redigering--papirkurv-tom` | Papirkurven er tom | `trash_list: []`, så klikk `library-trash-open` | [`no/redigering--papirkurv-tom.png`](no/redigering--papirkurv-tom.png) | [`en/redigering--papirkurv-tom.png`](en/redigering--papirkurv-tom.png) | — |
| `redigering--klipp` | Arbeidsflaten, steget «Klipp» — bølgeform og prekenforslag | `openEditorWithFile` på det fikstursydde opptaket | [`no/redigering--klipp.png`](no/redigering--klipp.png) | [`en/redigering--klipp.png`](en/redigering--klipp.png) | [`no/redigering--klipp--full.png`](no/redigering--klipp--full.png) · [`en/redigering--klipp--full.png`](en/redigering--klipp--full.png) · [`no/redigering--klipp--1000x760.png`](no/redigering--klipp--1000x760.png) |
| `redigering--klipp-leter` | «Leter etter prekenen …» — analysen er ikke ferdig | `editor_segments` som aldri svarer | [`no/redigering--klipp-leter.png`](no/redigering--klipp-leter.png) | [`en/redigering--klipp-leter.png`](en/redigering--klipp-leter.png) | — |
| `redigering--kuttliste` | Kuttlista — det som blir borte, som rader man kan angre | åpne, så klikk `editor-keep-sermon` | [`no/redigering--kuttliste.png`](no/redigering--kuttliste.png) | [`en/redigering--kuttliste.png`](en/redigering--kuttliste.png) | [`no/redigering--kuttliste--full.png`](no/redigering--kuttliste--full.png) · [`en/redigering--kuttliste--full.png`](en/redigering--kuttliste--full.png) |
| `redigering--lyd` | Steget «Lyd» — profilene og hva de gjør | åpne, så klikk `editor-steps-row-sound` | [`no/redigering--lyd.png`](no/redigering--lyd.png) | [`en/redigering--lyd.png`](en/redigering--lyd.png) | [`no/redigering--lyd--full.png`](no/redigering--lyd--full.png) · [`en/redigering--lyd--full.png`](en/redigering--lyd--full.png) |
| `redigering--mikser` | Mikseren åpen — de sju trinnene bak profilen | steget «Lyd», så `editor-mixer-open` + `editor-mixer-toggle` | [`no/redigering--mikser.png`](no/redigering--mikser.png) | [`en/redigering--mikser.png`](en/redigering--mikser.png) | [`no/redigering--mikser--full.png`](no/redigering--mikser--full.png) · [`en/redigering--mikser--full.png`](en/redigering--mikser--full.png) |
| `redigering--laster` | Arbeidsflaten laster — «Analyserer …» | `editor_load_recording` som aldri svarer | [`no/redigering--laster.png`](no/redigering--laster.png) | [`en/redigering--laster.png`](en/redigering--laster.png) | — |
| `redigering--feil` | Arbeidsflaten kunne ikke åpne fila | BÅDE `editor_load_recording: null` OG `editor_peaks: null` — toppene er en ANDRE kilde til varighet | [`no/redigering--feil.png`](no/redigering--feil.png) | [`en/redigering--feil.png`](en/redigering--feil.png) | — |

### Eksportering

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `eksport--tom` | Ingen fil åpen — siste opptak med én knapp, og en velger | `?goto=export` med tre opptak i lista | [`no/eksport--tom.png`](no/eksport--tom.png) | [`en/eksport--tom.png`](en/eksport--tom.png) | [`no/eksport--tom--1000x760.png`](no/eksport--tom--1000x760.png) |
| `eksport--ingenting` | Ingen opptak i det hele tatt | `recordings_list: []` | [`no/eksport--ingenting.png`](no/eksport--ingenting.png) | [`en/eksport--ingenting.png`](en/eksport--ingenting.png) | — |
| `eksport--valg` | Valgene — format, hvor, og hva som blir laget | `export-pick-use` på første rad i velgeren | [`no/eksport--valg.png`](no/eksport--valg.png) | [`en/eksport--valg.png`](en/eksport--valg.png) | — |
| `eksport--kjorer` | Eksporten kjører — 40 %, med avbryt | `editor_export` som henger (`EXPORT_HELD`) + `emit('editor-export-progress', {pct:40})` | [`no/eksport--kjorer.png`](no/eksport--kjorer.png) | [`en/eksport--kjorer.png`](en/eksport--kjorer.png) | — |
| `eksport--kvittering` | Kvitteringen — fila som ble laget, og hvor den ligger | `editor-export-go` med `editor_export` som svarer med én gang | [`no/eksport--kvittering.png`](no/eksport--kvittering.png) | [`en/eksport--kvittering.png`](en/eksport--kvittering.png) | — |

### Innstillinger

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `innstillinger--landing` | Hele flaten — kirkeprofil øverst, Avansert under | `?goto=settings:general` på en frisk maskin | [`no/innstillinger--landing.png`](no/innstillinger--landing.png) | [`en/innstillinger--landing.png`](en/innstillinger--landing.png) | [`no/innstillinger--landing--full.png`](no/innstillinger--landing--full.png) · [`en/innstillinger--landing--full.png`](en/innstillinger--landing--full.png) · [`no/innstillinger--landing--1000x760.png`](no/innstillinger--landing--1000x760.png) |
| `innstillinger--opptakskortet` | Avansert › Opptak, med alle betingede rader åpne | `stopOnSilence`, `splitMinutes` og `autoDeleteDays` alle satt | [`no/innstillinger--opptakskortet.png`](no/innstillinger--opptakskortet.png) | [`en/innstillinger--opptakskortet.png`](en/innstillinger--opptakskortet.png) | [`no/innstillinger--opptakskortet--full.png`](no/innstillinger--opptakskortet--full.png) · [`en/innstillinger--opptakskortet--full.png`](en/innstillinger--opptakskortet--full.png) |
| `innstillinger--systemkortet` | Avansert › System — diagnosedata, oppdatering, logg, profil, diagnose | rull til `advanced-system` | [`no/innstillinger--systemkortet.png`](no/innstillinger--systemkortet.png) | [`en/innstillinger--systemkortet.png`](en/innstillinger--systemkortet.png) | [`no/innstillinger--systemkortet--full.png`](no/innstillinger--systemkortet--full.png) · [`en/innstillinger--systemkortet--full.png`](en/innstillinger--systemkortet--full.png) |
| `innstillinger--oppdatering-klar` | Oppdateringsraden: en versjon er lastet ned og venter | `emit('update-downloaded', { version })` | [`no/innstillinger--oppdatering-klar.png`](no/innstillinger--oppdatering-klar.png) | [`en/innstillinger--oppdatering-klar.png`](en/innstillinger--oppdatering-klar.png) | [`no/innstillinger--oppdatering-klar--full.png`](no/innstillinger--oppdatering-klar--full.png) · [`en/innstillinger--oppdatering-klar--full.png`](en/innstillinger--oppdatering-klar--full.png) |
| `innstillinger--oppdatering-feilet` | Oppdateringsraden: sjekken gikk ikke | `emit('update-error', 'boom')` — en BAR streng, ikke et objekt | [`no/innstillinger--oppdatering-feilet.png`](no/innstillinger--oppdatering-feilet.png) | [`en/innstillinger--oppdatering-feilet.png`](en/innstillinger--oppdatering-feilet.png) | — |
| `innstillinger--telemetri-dialog` | «Vis» på diagnosedata — hva som faktisk sendes | `telemetry_preview_payload` + klikk `adv-diag-preview` | [`no/innstillinger--telemetri-dialog.png`](no/innstillinger--telemetri-dialog.png) | [`en/innstillinger--telemetri-dialog.png`](en/innstillinger--telemetri-dialog.png) | — |
| `innstillinger--smtp-uten-passord` | Varsling på e-post — ingen SMTP satt opp ennå | `email_has_smtp_password: false`, tomme SMTP-felter | [`no/innstillinger--smtp-uten-passord.png`](no/innstillinger--smtp-uten-passord.png) | [`en/innstillinger--smtp-uten-passord.png`](en/innstillinger--smtp-uten-passord.png) | [`no/innstillinger--smtp-uten-passord--full.png`](no/innstillinger--smtp-uten-passord--full.png) · [`en/innstillinger--smtp-uten-passord--full.png`](en/innstillinger--smtp-uten-passord--full.png) |
| `innstillinger--smtp-med-passord` | Varsling på e-post — passordet ligger i nøkkelringen | `email_has_smtp_password: true` + utfylte SMTP-felter | [`no/innstillinger--smtp-med-passord.png`](no/innstillinger--smtp-med-passord.png) | [`en/innstillinger--smtp-med-passord.png`](en/innstillinger--smtp-med-passord.png) | [`no/innstillinger--smtp-med-passord--full.png`](no/innstillinger--smtp-med-passord--full.png) · [`en/innstillinger--smtp-med-passord--full.png`](en/innstillinger--smtp-med-passord--full.png) |
| `innstillinger--tidsplan` | Tidsplanen — to faste tider, ett spesialopptak, vekking | `slots` + `specialRecordings` + `wake_capabilities` | [`no/innstillinger--tidsplan.png`](no/innstillinger--tidsplan.png) | [`en/innstillinger--tidsplan.png`](en/innstillinger--tidsplan.png) | [`no/innstillinger--tidsplan--full.png`](no/innstillinger--tidsplan--full.png) · [`en/innstillinger--tidsplan--full.png`](en/innstillinger--tidsplan--full.png) |
| `innstillinger--tidsplan-tom` | Tidsplanen — ingen faste tider ennå | `slots: []`, `specialRecordings: []` | [`no/innstillinger--tidsplan-tom.png`](no/innstillinger--tidsplan-tom.png) | [`en/innstillinger--tidsplan-tom.png`](en/innstillinger--tidsplan-tom.png) | [`no/innstillinger--tidsplan-tom--full.png`](no/innstillinger--tidsplan-tom--full.png) · [`en/innstillinger--tidsplan-tom--full.png`](en/innstillinger--tidsplan-tom--full.png) |

### Innstillinger › Diagnose

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `diagnose--hvile` | I hvile — «Kjør» og «Test-opptak», ingenting annet | diagnosen åpner en enhet, så den kjører aldri av seg selv | [`no/diagnose--hvile.png`](no/diagnose--hvile.png) | [`en/diagnose--hvile.png`](en/diagnose--hvile.png) | — |
| `diagnose--resultat` | Resultatet — fem statusrader, funn, enhetsliste, kopiknapp | `run_diagnostics` med tre funn, så klikk `adv-diagnose-run` | [`no/diagnose--resultat.png`](no/diagnose--resultat.png) | [`en/diagnose--resultat.png`](en/diagnose--resultat.png) | [`no/diagnose--resultat--full.png`](no/diagnose--resultat--full.png) · [`en/diagnose--resultat--full.png`](en/diagnose--resultat--full.png) |
| `diagnose--proven-hoppet-over` | Lydprøven ble ikke kjørt — den tredje tilstanden, ærlig | `captureOk: null` + `captureProbeSkipped` med motorens egen grunn | [`no/diagnose--proven-hoppet-over.png`](no/diagnose--proven-hoppet-over.png) | [`en/diagnose--proven-hoppet-over.png`](en/diagnose--proven-hoppet-over.png) | [`no/diagnose--proven-hoppet-over--full.png`](no/diagnose--proven-hoppet-over--full.png) · [`en/diagnose--proven-hoppet-over--full.png`](en/diagnose--proven-hoppet-over--full.png) |
| `diagnose--ipc-ring` | Kommandoer som ikke svarte denne økten | la `get_disk_space` og `recordings_list` kaste, så kjør | [`no/diagnose--ipc-ring.png`](no/diagnose--ipc-ring.png) | [`en/diagnose--ipc-ring.png`](en/diagnose--ipc-ring.png) | [`no/diagnose--ipc-ring--full.png`](no/diagnose--ipc-ring--full.png) · [`en/diagnose--ipc-ring--full.png`](en/diagnose--ipc-ring--full.png) |
| `diagnose--feilet` | Diagnosen kunne ikke kjøres | `run_diagnostics` kaster — den går utenom `call()`s fallback | [`no/diagnose--feilet.png`](no/diagnose--feilet.png) | [`en/diagnose--feilet.png`](en/diagnose--feilet.png) | — |
| `diagnose--testopptak` | Test-opptaket ble gjennomført | `run_test_recording` → `{ ok: true, signal: 'normal' }` | [`no/diagnose--testopptak.png`](no/diagnose--testopptak.png) | [`en/diagnose--testopptak.png`](en/diagnose--testopptak.png) | — |
| `diagnose--kopiert` | «Kopier full rapport» — kvitteringen som forsvinner av seg selv | stubbet utklippstavle + klikk `adv-diagnose-copy` | [`no/diagnose--kopiert.png`](no/diagnose--kopiert.png) | [`en/diagnose--kopiert.png`](en/diagnose--kopiert.png) | — |
| `diagnose--fra-menylinjen` | Menylinjens «Kjør diagnose» — bytter skjerm OG kjører | start på OPPTAK, `emitEvent('tray://action', 'run-diagnostics')` | [`no/diagnose--fra-menylinjen.png`](no/diagnose--fra-menylinjen.png) | [`en/diagnose--fra-menylinjen.png`](en/diagnose--fra-menylinjen.png) | [`no/diagnose--fra-menylinjen--full.png`](no/diagnose--fra-menylinjen--full.png) · [`en/diagnose--fra-menylinjen--full.png`](en/diagnose--fra-menylinjen--full.png) |

### Første gang

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `forste-gang--1-lyd-lukket` | Steg 1 av 5 — lydporten er LUKKET: «Neste» venter på lyd | `onboardingDone:false`, ingen VU-pakker | [`no/forste-gang--1-lyd-lukket.png`](no/forste-gang--1-lyd-lukket.png) | [`en/forste-gang--1-lyd-lukket.png`](en/forste-gang--1-lyd-lukket.png) | [`no/forste-gang--1-lyd-lukket--1000x760.png`](no/forste-gang--1-lyd-lukket--1000x760.png) |
| `forste-gang--1-lyd-apen` | Steg 1 av 5 — lydporten er ÅPEN: vi hører noe | `emit('vu-levels', { peak_dbfs: [-20,-20] })` — over −50 dBFS | [`no/forste-gang--1-lyd-apen.png`](no/forste-gang--1-lyd-apen.png) | [`en/forste-gang--1-lyd-apen.png`](en/forste-gang--1-lyd-apen.png) | — |
| `forste-gang--2-mappe` | Steg 2 av 5 — hvor skal opptakene? | «Fortsett uten lyd», så ett steg fram | [`no/forste-gang--2-mappe.png`](no/forste-gang--2-mappe.png) | [`en/forste-gang--2-mappe.png`](en/forste-gang--2-mappe.png) | — |
| `forste-gang--3-kvalitet` | Steg 3 av 5 — hvilken kvalitet? | «Fortsett uten lyd», så to steg fram | [`no/forste-gang--3-kvalitet.png`](no/forste-gang--3-kvalitet.png) | [`en/forste-gang--3-kvalitet.png`](en/forste-gang--3-kvalitet.png) | — |
| `forste-gang--4-kirke` | Steg 4 av 5 — hva heter menigheten? | «Fortsett uten lyd», så tre steg fram | [`no/forste-gang--4-kirke.png`](no/forste-gang--4-kirke.png) | [`en/forste-gang--4-kirke.png`](en/forste-gang--4-kirke.png) | — |
| `forste-gang--5-varsling` | Steg 5 av 5 — hvem skal få beskjed? | «Fortsett uten lyd», så fire steg fram | [`no/forste-gang--5-varsling.png`](no/forste-gang--5-varsling.png) | [`en/forste-gang--5-varsling.png`](en/forste-gang--5-varsling.png) | [`no/forste-gang--5-varsling--full.png`](no/forste-gang--5-varsling--full.png) · [`en/forste-gang--5-varsling--full.png`](en/forste-gang--5-varsling--full.png) |
| `forste-gang--6-sjekkliste` | «Klar til søndag» — de fem svarene, med det som mangler i gult | «Fortsett uten lyd» + fire steg; `notify` er ikke satt opp | [`no/forste-gang--6-sjekkliste.png`](no/forste-gang--6-sjekkliste.png) | [`en/forste-gang--6-sjekkliste.png`](en/forste-gang--6-sjekkliste.png) | [`no/forste-gang--6-sjekkliste--full.png`](no/forste-gang--6-sjekkliste--full.png) · [`en/forste-gang--6-sjekkliste--full.png`](en/forste-gang--6-sjekkliste--full.png) · [`no/forste-gang--6-sjekkliste--1000x760.png`](no/forste-gang--6-sjekkliste--1000x760.png) |

### Skallet

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `skallet--status-ingen-kilde` | Statuslinja: «ingen lydkilde valgt» (gul) | ingen `deviceId` | [`no/skallet--status-ingen-kilde.png`](no/skallet--status-ingen-kilde.png) | [`en/skallet--status-ingen-kilde.png`](en/skallet--status-ingen-kilde.png) | — |
| `skallet--status-lite-plass` | Statuslinja: «lite plass igjen» (gul) — slår «ingen kilde» | `freeBytes = 200 MB` og ingen valgt kilde samtidig | [`no/skallet--status-lite-plass.png`](no/skallet--status-lite-plass.png) | [`en/skallet--status-lite-plass.png`](en/skallet--status-lite-plass.png) | — |
| `skallet--status-neste` | Statuslinja: «neste opptak …» (grå) | kilde valgt + `autoRecordEnabled` + `scheduler_status.next` | [`no/skallet--status-neste.png`](no/skallet--status-neste.png) | [`en/skallet--status-neste.png`](en/skallet--status-neste.png) | — |
| `skallet--status-klar` | Statuslinja: «alt er klart» (grønn) | kilde valgt, plass på disken, ingenting planlagt | [`no/skallet--status-klar.png`](no/skallet--status-klar.png) | [`en/skallet--status-klar.png`](en/skallet--status-klar.png) | — |
| `skallet--oppdateringsbanner` | Oppdateringsstripa — over den siden man ER på | `emit('update-available', { version })` på Redigering | [`no/skallet--oppdateringsbanner.png`](no/skallet--oppdateringsbanner.png) | [`en/skallet--oppdateringsbanner.png`](en/skallet--oppdateringsbanner.png) | — |
| `skallet--hydreringsfeil` | Innstillingene kunne ikke leses — aldri stille standardverdier | `settings_get` kaster; feilstripa står under overskriften | [`no/skallet--hydreringsfeil.png`](no/skallet--hydreringsfeil.png) | [`en/skallet--hydreringsfeil.png`](en/skallet--hydreringsfeil.png) | — |

## Hvordan scenene lages

Scenetabellen ligger i `e2e/atlas/scenes.ts`. Hver scene er
`{ fixtures, settings, goto }` gjennom `e2e/harness.ts` (api-shim-sømmen),
pluss eventuelle klikk mot `data-testid`. `e2e/atlas/harness.ts` legger til
det den vanlige nettleser-tieren ikke har:

1. **Backend-hendelser.** En stor del av tilstandene males av en Tauri-event,
   ikke av et kall: opptaksmåleren (`recording://levels`), tapt-opptak-kortet
   (`scheduler://missed`), forhåndssjekken (`scheduler://preflight`),
   nedtellingen før auto-stopp, oppdateringsbanneret. Brua er
   `e2e/events.ts` — den samme de vanlige spec-ene bruker — og den må
   installeres FØR `boot()`. `emit`/`emitEvent` returnerer hvor mange
   lyttere som tok imot; **0 lyttere feiler scenen**, slik at atlaset ikke
   fotograferer feil skjerm i stillhet.
2. **En fast klokke.** `page.clock.setFixedTime` låser `Date.now()` til
   søndag 23. august 2026 kl. 10:55, slik at «om 3 dager», «for 2 timer siden»
   og opptakstelleren er de samme i to kjøringer.
3. **`settle()`.** `toBeVisible()` betyr IKKE «malt»: Playwrights synlighet
   er boks + `display`/`visibility`, og sier ingenting om OPACITY. Så
   fotografen venter til hver endelige animasjon og overgang er ferdig
   (uendelige — spinnere — er unntatt, ellers ville en travel skjerm aldri falt
   til ro), og deretter én `requestAnimationFrame` for rendererens egne
   malere (VU-barene, waveform-canvaset).
4. **VU-pakker som har konvergert.** `settleVu` sender identiske pakker til
   utjevningen har stabilisert seg og stopper — måleren holder siste malte
   posisjon, fordi det er pakkene som driver malingen.
