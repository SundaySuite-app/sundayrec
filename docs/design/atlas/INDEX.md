# Atlas — SundayRec slik appen ER

Hvert bilde er ett skjermbilde av appen slik den står i **v0.32.0-beta.1 — «én flate»**:
to flater (**Ta opp** og **Opptak**) og et tannhjul. Alt som før var egne sider er
flyttet dit det brukes — lydkilde, mappe og kvalitet i **lydarket**, planen og
bryterne i **ukestripa** nederst på Ta opp, formatet og intro/outro i
**eksportarket**, språk, oppdatering og statistikk i **tannhjulet** — og opptaket
selv er «bare opptaket» (**opptaksmodus** med hold-for-stopp). Skissen som ble
bygget etter ligger i [`../en-flate/`](../en-flate/). Ingenting her er et
forslag — dette er den visuelle regresjonsbasen. Scene-id-ene som het
`innstillinger--*` og `plan--*` er erstattet av `tannhjul--*`,
`lydark--*` og `strip--*`; de som lever (`opptak--*`, `redigering--*`,
`eksport--*`, `forste-gang--*`, `skallet--*`) beholder navnet.

Arkivet fra v0.15 ligger i [`../atlas-v015/`](../atlas-v015/INDEX.md), sammen
med IA-uttrekket i [`../ATLAS.md`](../ATLAS.md). De to viser appen slik den
VAR, og skal ikke forveksles med denne mappa.

**Vindu:** 1180×760 (Tauri-vinduets standardstørrelse). Hovedscenene er i
tillegg fotografert på 1000×760 — en smalere maskin enn standard — med suffikset
`--1000x760` (norsk bare: om en layout overlever bredden er ikke et
språkspørsmål). Sider som ruller forbi vindushøyden har
et `--full`-skudd som viser hele siden (vindushøyden settes til innholdets
høyde; Playwrights egen `fullPage` gir ingenting her, fordi appen ruller inne i
`#main`, ikke i dokumentet).

**Språk:** `no/` og `en/`. De fem andre språkene i språkvelgeren er satt på
pause og er ikke fotografert.

**Total størrelse:** 4.0 MB i 149 PNG-er. Komprimert med `magick mogrify -colors 256`: 10.8 MB → 4.0 MB.

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

### Ta opp

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `opptak--kald` | Kald start — ingen lydkilde valgt ennå | ingen `deviceId`; den store knappen er sperret og sier hvorfor | [`no/opptak--kald.png`](no/opptak--kald.png) | [`en/opptak--kald.png`](en/opptak--kald.png) | [`no/opptak--kald--1000x760.png`](no/opptak--kald--1000x760.png) |
| `opptak--klar` | Klar — kilde-brikka med L/R-måler, den store knappen, og ukestripa med «Neste» | `deviceId:x32` + én fast tid + `scheduler_status.next`; måleren settles | [`no/opptak--klar.png`](no/opptak--klar.png) | [`en/opptak--klar.png`](en/opptak--klar.png) | [`no/opptak--klar--1000x760.png`](no/opptak--klar--1000x760.png) |
| `opptak--kilde-borte` | Kilden er valgt, men ikke til stede nå | `list_audio_devices` uten `x32`; Start er ÅPEN, med advarsel | [`no/opptak--kilde-borte.png`](no/opptak--kilde-borte.png) | [`en/opptak--kilde-borte.png`](en/opptak--kilde-borte.png) | — |
| `opptak--kamera-live` | Kamera på — bildet fyller flaten, med kameranavn, måler i kanten og liten opptaksknapp | `videoEnabled:true` + `camera_preview_start`-fixtur og en ekte JPEG ned kanalen | [`no/opptak--kamera-live.png`](no/opptak--kamera-live.png) | [`en/opptak--kamera-live.png`](en/opptak--kamera-live.png) | — |
| `opptak--kamera-valg` | Kamera på — popoveren bak den lille pila: kamera og egen lydfil | som over, så klikk `record-camera-more` | [`no/opptak--kamera-valg.png`](no/opptak--kamera-valg.png) | [`en/opptak--kamera-valg.png`](en/opptak--kamera-valg.png) | — |
| `opptak--kamera-nektet` | Kamerabildet — fasen `denied` (OS-et sa nei) | `camera_preview_start` som svarer «camera access is not allowed» | [`no/opptak--kamera-nektet.png`](no/opptak--kamera-nektet.png) | [`en/opptak--kamera-nektet.png`](en/opptak--kamera-nektet.png) | — |
| `opptak--kamera-borte` | Kamerabildet — det lagrede kameraet finnes ikke | `videoDeviceName` som ikke er i bakendens kameraliste | [`no/opptak--kamera-borte.png`](no/opptak--kamera-borte.png) | [`en/opptak--kamera-borte.png`](en/opptak--kamera-borte.png) | — |
| `opptak--banner-avbrutt` | Banner: opptaket ble avbrutt, med grunnen i klartekst | `emit('recording-error', { code: 'device_disconnected' })` | [`no/opptak--banner-avbrutt.png`](no/opptak--banner-avbrutt.png) | [`en/opptak--banner-avbrutt.png`](en/opptak--banner-avbrutt.png) | — |
| `opptak--banner-lite-plass` | Banner: under to timer igjen på disken | `get_disk_space.freeBytes = 200 MB` | [`no/opptak--banner-lite-plass.png`](no/opptak--banner-lite-plass.png) | [`en/opptak--banner-lite-plass.png`](en/opptak--banner-lite-plass.png) | — |
| `opptak--banner-gikk-glipp` | Banner: et planlagt opptak ble aldri tatt | `emitEvent('scheduler://missed', [{ at, label }])` | [`no/opptak--banner-gikk-glipp.png`](no/opptak--banner-gikk-glipp.png) | [`en/opptak--banner-gikk-glipp.png`](en/opptak--banner-gikk-glipp.png) | — |
| `opptak--banner-forhandssjekk` | Banner: forhåndssjekken fant noe å se på | `media_permissions.microphone = denied` + ett `run_preflight`-funn | [`no/opptak--banner-forhandssjekk.png`](no/opptak--banner-forhandssjekk.png) | [`en/opptak--banner-forhandssjekk.png`](en/opptak--banner-forhandssjekk.png) | — |
| `opptak--samtykkekort` | Samtykkekortet — det ene spørsmålet om diagnosedata | `telemetry_consent_get.needsPrompt = true` | [`no/opptak--samtykkekort.png`](no/opptak--samtykkekort.png) | [`en/opptak--samtykkekort.png`](en/opptak--samtykkekort.png) | — |
| `opptak--kvittering` | Kvitteringen etter et ferdig opptak | `emit('recording-finished', { path })` | [`no/opptak--kvittering.png`](no/opptak--kvittering.png) | [`en/opptak--kvittering.png`](en/opptak--kvittering.png) | — |

### Lydarket

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `lydark--enhet` | Lydarket — enhetskort, kanalpar og stor L/R-måler | `?goto=settings:sound` → `record` + panelet `source` | [`no/lydark--enhet.png`](no/lydark--enhet.png) | [`en/lydark--enhet.png`](en/lydark--enhet.png) | — |
| `lydark--mer` | «Mer» utfoldet — mappe, kvalitet og de fem flisene (Før, Stille, Del, Rydd, Maks) | `?goto=settings:engine` → panelet `source-more`; alle fem flisene på | [`no/lydark--mer.png`](no/lydark--mer.png) | [`en/lydark--mer.png`](en/lydark--mer.png) | — |
| `lydark--mer-flisene-av` | «Mer» utfoldet — alle fem flisene av, «Av» står under dem | som over, med alle fem verdiene på 0 | [`no/lydark--mer-flisene-av.png`](no/lydark--mer-flisene-av.png) | [`en/lydark--mer-flisene-av.png`](en/lydark--mer-flisene-av.png) | — |

### Ukestripa

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `strip--pille` | Uten faste tider: bare den lille kalenderknappen nede til venstre, med gullplussen | `slots: []` | [`no/strip--pille.png`](no/strip--pille.png) | [`en/strip--pille.png`](en/strip--pille.png) | — |
| `strip--lukket` | Lukket stripe — mini-uke, «Neste: søndag 11:00», nedtelling og de fire statusikonene | én fast tid + `scheduler_status.next`, Auto på | [`no/strip--lukket.png`](no/strip--lukket.png) | [`en/strip--lukket.png`](en/strip--lukket.png) | — |
| `strip--lukket-auto-av` | Lukket stripe med tider men Auto AV — dempet, «Automatisk opptak er av» | `slots` satt, `autoRecordEnabled:false` | [`no/strip--lukket-auto-av.png`](no/strip--lukket-auto-av.png) | [`en/strip--lukket-auto-av.png`](en/strip--lukket-auto-av.png) | — |
| `strip--apen` | Åpen stripe — de fire flisene og ukevisningen, «Neste» fremhevet, ett opptak denne uka ikke tatt opp | `?goto=plan`; tre tider, `scheduler_status.next` på søndagen, `emitEvent('scheduler://missed')` | [`no/strip--apen.png`](no/strip--apen.png) | [`en/strip--apen.png`](en/strip--apen.png) | — |
| `strip--blokk` | Åpen stripe — redigeringen åpen ved siden av en valgt blokk | som over, så et trykk på søndagsblokka | [`no/strip--blokk.png`](no/strip--blokk.png) | [`en/strip--blokk.png`](en/strip--blokk.png) | — |
| `strip--tom-uke` | Åpen stripe med tom uke — «Hold og dra», ingen faste tider ennå | `?goto=plan`, `slots: []` | [`no/strip--tom-uke.png`](no/strip--tom-uke.png) | [`en/strip--tom-uke.png`](en/strip--tom-uke.png) | — |
| `strip--auto-uten-tider` | Auto på i en tom uke — flisen lyser ikke og sier «Ingen tider» (ingen tid er funnet på) | `?goto=plan`, `autoRecordEnabled:true`, `slots: []` | [`no/strip--auto-uten-tider.png`](no/strip--auto-uten-tider.png) | [`en/strip--auto-uten-tider.png`](en/strip--auto-uten-tider.png) | — |
| `strip--vekking` | Vekk-flisens popover — hva maskinen kan, «Legg inn vekking», prøv og logg | `wake_capabilities`; klikk hjørneknappen `strip-wake-more` | [`no/strip--vekking.png`](no/strip--vekking.png) | [`en/strip--vekking.png`](en/strip--vekking.png) | — |
| `strip--varsel` | Varsel-flisens popover — status fra systemet og «Send testvarsel» | `notification_permission:granted`; klikk `strip-notify-more`, så «Send testvarsel» | [`no/strip--varsel.png`](no/strip--varsel.png) | [`en/strip--varsel.png`](en/strip--varsel.png) | — |
| `strip--varsel-av-i-systemet` | Varsler slått på, men av i systemet — stripa sier fra og tilbyr systeminnstillingene | `notification_permission:denied` + klikk på Varsel-flisen | [`no/strip--varsel-av-i-systemet.png`](no/strip--varsel-av-i-systemet.png) | [`en/strip--varsel-av-i-systemet.png`](en/strip--varsel-av-i-systemet.png) | — |

### Opptaksmodus

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `opptaksmodus--lyd` | Det tas opp uten kamera — enorm klokke, bred L/R-måler, «Hold» og «Til du stopper» | `emit('recording-overlay-stop', {state:'recording'})`, klokka +42 min | [`no/opptaksmodus--lyd.png`](no/opptaksmodus--lyd.png) | [`en/opptaksmodus--lyd.png`](en/opptaksmodus--lyd.png) | — |
| `opptaksmodus--kamera` | Det tas opp med kamera — bildet i ramme med rød kant, klokka som brikke, loddrett måler | som over med `videoEnabled`, og en ekte JPEG ned kamerakanalen FØR opptaket (overlegget arver bildet) | [`no/opptaksmodus--kamera.png`](no/opptaksmodus--kamera.png) | [`en/opptaksmodus--kamera.png`](en/opptaksmodus--kamera.png) | — |
| `opptaksmodus--stopper-om` | Planlagt stopp om 15 minutter — «Stopper 11:52» med «+30» og «∞» | `scheduled_stop_ms` = klokka + 57 min, så klokka flyttes til +42 | [`no/opptaksmodus--stopper-om.png`](no/opptaksmodus--stopper-om.png) | [`en/opptaksmodus--stopper-om.png`](en/opptaksmodus--stopper-om.png) | — |
| `opptaksmodus--stillhet` | Stillhetsvarsel som liten ikon + ord-brikke øverst | overlegget oppe, så `emit('recording-silence', …)` | [`no/opptaksmodus--stillhet.png`](no/opptaksmodus--stillhet.png) | [`en/opptaksmodus--stillhet.png`](en/opptaksmodus--stillhet.png) | — |
| `opptaksmodus--skjermleser-stopp` | Stopp-bekreftelsen som VoiceOver/Narrator får i stedet for holdet — «Fortsett å ta opp» er primærvalget | overlegget oppe, så et `click` uten pekertrykk (`detail: 0`) på `overlay-stop` | [`no/opptaksmodus--skjermleser-stopp.png`](no/opptaksmodus--skjermleser-stopp.png) | [`en/opptaksmodus--skjermleser-stopp.png`](en/opptaksmodus--skjermleser-stopp.png) | — |

### Opptak og redigering

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `redigering--bibliotek` | Lista med opptak — mini-bølgeform, dato, lengde, notat | tre rader i `recordings_list`, én med notat | [`no/redigering--bibliotek.png`](no/redigering--bibliotek.png) | [`en/redigering--bibliotek.png`](en/redigering--bibliotek.png) | [`no/redigering--bibliotek--1000x760.png`](no/redigering--bibliotek--1000x760.png) |
| `redigering--bibliotek-tomt` | Lista er tom — og sier hva man gjør nå | `recordings_list: []` | [`no/redigering--bibliotek-tomt.png`](no/redigering--bibliotek-tomt.png) | [`en/redigering--bibliotek-tomt.png`](en/redigering--bibliotek-tomt.png) | — |
| `redigering--sok-ingen-treff` | Søket ga ingen treff | fyll `library-search` med «finnesikke» | [`no/redigering--sok-ingen-treff.png`](no/redigering--sok-ingen-treff.png) | [`en/redigering--sok-ingen-treff.png`](en/redigering--sok-ingen-treff.png) | — |
| `redigering--papirkurv` | Papirkurven (ikon oppe til høyre, med antall) — med «tømmes om N d» | to rader i `trash_list`, så klikk søppelikonet `library-trash-open` | [`no/redigering--papirkurv.png`](no/redigering--papirkurv.png) | [`en/redigering--papirkurv.png`](en/redigering--papirkurv.png) | [`no/redigering--papirkurv--full.png`](no/redigering--papirkurv--full.png) · [`en/redigering--papirkurv--full.png`](en/redigering--papirkurv--full.png) |
| `redigering--papirkurv-tom` | Papirkurven er tom | `trash_list: []`, så klikk søppelikonet `library-trash-open` | [`no/redigering--papirkurv-tom.png`](no/redigering--papirkurv-tom.png) | [`en/redigering--papirkurv-tom.png`](en/redigering--papirkurv-tom.png) | — |
| `redigering--klipp` | Redigering, steget «Klipp» (lydopptak) — bølgeform med to håndtak, ingen video | `openEditorWithRecording` på det fikstursydde opptaket | [`no/redigering--klipp.png`](no/redigering--klipp.png) | [`en/redigering--klipp.png`](en/redigering--klipp.png) | [`no/redigering--klipp--full.png`](no/redigering--klipp--full.png) · [`en/redigering--klipp--full.png`](en/redigering--klipp--full.png) · [`no/redigering--klipp--1000x760.png`](no/redigering--klipp--1000x760.png) |
| `redigering--markering` | Dra i bølgeformen — markering med boblen «Klipp» og lengden | åpne, så dra over bølgeformen fra 30 % til 40 % | [`no/redigering--markering.png`](no/redigering--markering.png) | [`en/redigering--markering.png`](en/redigering--markering.png) | [`no/redigering--markering--full.png`](no/redigering--markering--full.png) · [`en/redigering--markering--full.png`](en/redigering--markering--full.png) |
| `redigering--kuttliste` | Et klipp er gjort — skravert område med angre-knapp, og kuttlista under | åpne, dra en markering, trykk «Klipp» | [`no/redigering--kuttliste.png`](no/redigering--kuttliste.png) | [`en/redigering--kuttliste.png`](en/redigering--kuttliste.png) | [`no/redigering--kuttliste--full.png`](no/redigering--kuttliste--full.png) · [`en/redigering--kuttliste--full.png`](en/redigering--kuttliste--full.png) |
| `redigering--stillhetsforslag` | Stillhetsforslag — stiplede områder med «Godta» og «Avvis»; ingenting er klippet | `editor_peaks` med to stillheter, så klikk `editor-silence` | [`no/redigering--stillhetsforslag.png`](no/redigering--stillhetsforslag.png) | [`en/redigering--stillhetsforslag.png`](en/redigering--stillhetsforslag.png) | [`no/redigering--stillhetsforslag--full.png`](no/redigering--stillhetsforslag--full.png) · [`en/redigering--stillhetsforslag--full.png`](en/redigering--stillhetsforslag--full.png) |
| `redigering--video` | Redigering av et videoopptak — stor 16:9-ramme med spill, tid og Inn/Ut, bølgeformen under | `editor_load_recording.hasVideo = true`; `<video>` har ingen fil å vise i nettleseren, så rammen står svart | [`no/redigering--video.png`](no/redigering--video.png) | [`en/redigering--video.png`](en/redigering--video.png) | [`no/redigering--video--full.png`](no/redigering--video--full.png) · [`en/redigering--video--full.png`](en/redigering--video--full.png) |
| `redigering--lyd` | Steget «Lyd» — bryteren «Podkastklar lyd» | åpne, så klikk `editor-steps-row-sound` | [`no/redigering--lyd.png`](no/redigering--lyd.png) | [`en/redigering--lyd.png`](en/redigering--lyd.png) | — |
| `redigering--laster` | Arbeidsflaten laster — «Analyserer …» | `editor_load_recording` som aldri svarer | [`no/redigering--laster.png`](no/redigering--laster.png) | [`en/redigering--laster.png`](en/redigering--laster.png) | — |
| `redigering--feil` | Arbeidsflaten kunne ikke åpne fila | BÅDE `editor_load_recording: null` OG `editor_peaks: null` — toppene er en ANDRE kilde til varighet | [`no/redigering--feil.png`](no/redigering--feil.png) | [`en/redigering--feil.png`](en/redigering--feil.png) | — |

### Eksportarket

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `eksport--valg` | Eksportarket (sideark fra høyre) — formatkortene MP3 «Podkast» og WAV «Arkiv», de tre flisene, mappe og «Lagre» | åpne opptaket, så klikk `editor-export-open` | [`no/eksport--valg.png`](no/eksport--valg.png) | [`en/eksport--valg.png`](en/eksport--valg.png) | — |
| `eksport--video` | Eksportarket for et videoopptak — MP4 «Video» er med som tredje kort; med det valgt står intro/outro dempet | `hasVideo = true`, så klikk MP4-kortet | [`no/eksport--video.png`](no/eksport--video.png) | [`en/eksport--video.png`](en/eksport--video.png) | — |
| `eksport--jingler` | Intro og outro valgt — filnavn på flisene | innstillingene har `editorIntroPath` og `editorOutroPath`; åpne opptaket, så klikk `editor-export-open` | [`no/eksport--jingler.png`](no/eksport--jingler.png) | [`en/eksport--jingler.png`](en/eksport--jingler.png) | — |
| `eksport--jingle-meny` | Hjørneknappen på Intro-flisen — «Velg en annen fil» og «Fjern» | som over, så klikk `export-jingle-intro-more` | [`no/eksport--jingle-meny.png`](no/eksport--jingle-meny.png) | [`en/eksport--jingle-meny.png`](en/eksport--jingle-meny.png) | — |
| `eksport--kjorer` | Eksporten kjører — 40 %, med avbryt | `editor_export` som henger (`EXPORT_HELD`) + `emit('editor-export-progress', {pct:40})` | [`no/eksport--kjorer.png`](no/eksport--kjorer.png) | [`en/eksport--kjorer.png`](en/eksport--kjorer.png) | — |
| `eksport--kvittering` | Kvitteringen — fila som ble laget, og hvor den ligger | `editor-export-go` med `editor_export` som svarer med én gang | [`no/eksport--kvittering.png`](no/eksport--kvittering.png) | [`en/eksport--kvittering.png`](en/eksport--kvittering.png) | — |

### Tannhjulet

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `tannhjul--kort` | Tannhjulet åpent over siden man kom fra — Språk, Oppdatering, Anonym statistikk og Om | `?goto=settings` (kortet ligger over `underlay`) | [`no/tannhjul--kort.png`](no/tannhjul--kort.png) | [`en/tannhjul--kort.png`](en/tannhjul--kort.png) | [`no/tannhjul--kort--1000x760.png`](no/tannhjul--kort--1000x760.png) |
| `tannhjul--oppdatering-klar` | Oppdateringsraden: en versjon er lastet ned og venter | `emit('update-downloaded', { version })` | [`no/tannhjul--oppdatering-klar.png`](no/tannhjul--oppdatering-klar.png) | [`en/tannhjul--oppdatering-klar.png`](en/tannhjul--oppdatering-klar.png) | — |
| `tannhjul--oppdatering-feilet` | Oppdateringsraden: sjekken gikk ikke | `emit('update-error', 'boom')` — en BAR streng, ikke et objekt | [`no/tannhjul--oppdatering-feilet.png`](no/tannhjul--oppdatering-feilet.png) | [`en/tannhjul--oppdatering-feilet.png`](en/tannhjul--oppdatering-feilet.png) | — |
| `tannhjul--statistikk` | «Vis hva som sendes» på anonym statistikk — hele nyttelasten | `telemetry_preview_payload` + klikk `adv-diag-preview` | [`no/tannhjul--statistikk.png`](no/tannhjul--statistikk.png) | [`en/tannhjul--statistikk.png`](en/tannhjul--statistikk.png) | — |
| `tannhjul--lisenser` | Om › «Vis lisenser» — de medfølgende lisensene i en ramme | `licenses_notices` med en kort tekst, så klikk `adv-licenses-open` | [`no/tannhjul--lisenser.png`](no/tannhjul--lisenser.png) | [`en/tannhjul--lisenser.png`](en/tannhjul--lisenser.png) | — |

### Første gang

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `forste-gang--1-lyd-lukket` | Hint 1 av 3 under kilde-brikka — lydporten er LUKKET: «Neste» venter på lyd | `onboardingDone:false`, ingen VU-pakker | [`no/forste-gang--1-lyd-lukket.png`](no/forste-gang--1-lyd-lukket.png) | [`en/forste-gang--1-lyd-lukket.png`](en/forste-gang--1-lyd-lukket.png) | [`no/forste-gang--1-lyd-lukket--1000x760.png`](no/forste-gang--1-lyd-lukket--1000x760.png) |
| `forste-gang--1-lyd-apen` | Hint 1 av 3 — lydporten er ÅPEN: vi hører noe | `emit('vu-levels', { peak_dbfs: [-20,-20] })` — over −50 dBFS | [`no/forste-gang--1-lyd-apen.png`](no/forste-gang--1-lyd-apen.png) | [`en/forste-gang--1-lyd-apen.png`](en/forste-gang--1-lyd-apen.png) | — |
| `forste-gang--2-mappe` | Hint 2 av 3 — hvor skal opptakene? (lydarket åpnes med «Mer», mappe-raden pulserer) | «Fortsett uten lyd», så ett steg fram | [`no/forste-gang--2-mappe.png`](no/forste-gang--2-mappe.png) | [`en/forste-gang--2-mappe.png`](en/forste-gang--2-mappe.png) | — |
| `forste-gang--3-plan` | Hint 3 av 3 — faste opptak, over kalenderknappen | «Fortsett uten lyd», så to steg fram | [`no/forste-gang--3-plan.png`](no/forste-gang--3-plan.png) | [`en/forste-gang--3-plan.png`](en/forste-gang--3-plan.png) | [`no/forste-gang--3-plan--1000x760.png`](no/forste-gang--3-plan--1000x760.png) |

### Skallet

| Scene-id | Tilstand | Oppskrift | no | en | Ekstra |
| --- | --- | --- | --- | --- | --- |
| `skallet--status-ingen-kilde` | Statuslinja: «ingen lydkilde valgt» (gul) | ingen `deviceId` | [`no/skallet--status-ingen-kilde.png`](no/skallet--status-ingen-kilde.png) | [`en/skallet--status-ingen-kilde.png`](en/skallet--status-ingen-kilde.png) | — |
| `skallet--status-lite-plass` | Statuslinja: «lite plass igjen» (gul) — slår «ingen kilde» | `freeBytes = 200 MB` og ingen valgt kilde samtidig | [`no/skallet--status-lite-plass.png`](no/skallet--status-lite-plass.png) | [`en/skallet--status-lite-plass.png`](en/skallet--status-lite-plass.png) | — |
| `skallet--status-neste` | Statuslinja: «neste opptak …» (grå) | kilde valgt + `autoRecordEnabled` + `scheduler_status.next` | [`no/skallet--status-neste.png`](no/skallet--status-neste.png) | [`en/skallet--status-neste.png`](en/skallet--status-neste.png) | — |
| `skallet--status-klar` | Statuslinja: «alt er klart» (grønn) | kilde valgt, plass på disken, ingenting planlagt | [`no/skallet--status-klar.png`](no/skallet--status-klar.png) | [`en/skallet--status-klar.png`](en/skallet--status-klar.png) | — |
| `skallet--oppdateringsbanner` | Oppdateringsstripa — over den siden man ER på | `emit('update-available', { version })` på Opptak | [`no/skallet--oppdateringsbanner.png`](no/skallet--oppdateringsbanner.png) | [`en/skallet--oppdateringsbanner.png`](en/skallet--oppdateringsbanner.png) | — |
| `skallet--hydreringsfeil` | Innstillingene kunne ikke leses — aldri stille standardverdier | `settings_get` kaster; feilstripa står under overskriften | [`no/skallet--hydreringsfeil.png`](no/skallet--hydreringsfeil.png) | [`en/skallet--hydreringsfeil.png`](en/skallet--hydreringsfeil.png) | — |

## Riggtest før neste slipp

Det som bare en ekte maskin kan bevise står i [`docs/PLAN.md`](../../PLAN.md)
under «Åpent — rigg trengs» og i [`docs/SMOKE-TEST.md`](../../SMOKE-TEST.md).
For «én flate» er det særlig: kamerabildet på 1280 px / 12 bilder i sekundet på
en svak Windows-PC, H.264-avspilling i redigeringen (WKWebView og WebView2),
hold-for-stopp med VoiceOver og Narrator, første gang på en fersk maskin, og
planlagt opptak med «+30» og «∞» på ekte utstyr. Teksten på bildene her skal i
tillegg leses av noen som snakker språket (no og en).

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
