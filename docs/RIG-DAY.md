# Riggdag — sjekkliste

En sittende gjennomgang av det som `npm run check` strukturelt ikke kan se:
ekte maskinvare, ekte krasj, ekte klokketid. `docs/NEEDS-RICHARD.md`s
**HARDWARE-UNVERIFIED**-liste sier HVA som mangler rigg-bevis; denne sida er
HVORDAN — én økt, i denne rekkefølgen, så dagen ikke blir å finne opp
punktene på nytt hver gang. Sett av en hel dag: flere av punktene krever
ventetid (planlagte slot, søvn/vekk-sykluser, en 90-minutters opptak) som
ikke lar seg presse sammen.

**Forutsetninger:** en Mac og en Windows-boks, begge med SundayRec
installert og et ekte lydoppsett (USB-mikrofon eller mikser) tilkoblet;
tilgang til terminal på Mac-en (for `kill -9`) og til Oppgavebehandling +
`%APPDATA%`- og `%LOCALAPPDATA%`-mappene på Windows-boksen; en kopi-vennlig ekte
`sundayrec.sqlite` det er greit å teste mot; nok tid til at maskinen kan
sovne og våkne av seg selv minst én gang.

Kryss av etter hvert. Et punkt som IKKE stemmer med forventet resultat er en
feilrapport, ikke en avkrysning — noter det og fortsett til neste; ikke la
ett rødt punkt stoppe resten av dagen. Der et punkt er født av en bestemt
F2-fiks (PR-nummer i parentes), står forventet resultat **FØR** fiksen først
(så du vet hva et regresjonsfunn ville sett ut som) og **ETTER** — det du
faktisk skal se nå — sist.

## Mac-boksen

- [ ] **(a) Trekk mikseren midt i opptaket.** Start et opptak, la det gå et
      minutt, trekk ut USB-kabelen til mikseren/mikrofonen og **la den stå
      ute i over 60 sekunder** før du kobler den til igjen.
      **Forventet:** opptaksoverlegget blir stående med en
      gjenkoblingsstripe (ikke en feilmelding), og når enheten kobles til
      igjen fortsetter samme opptak — ÉN fil etterpå, ingen splitt. Ingen
      system-varsel underveis: dette er en advarsel,
      ikke en feil, så lenge motoren får koblet til igjen.
- [ ] **(a, fortsettelse) Sett den ALDRI tilbake.** Gjenta med en ny
      opptaksøkt, men denne gangen: la mikseren stå frakoblet. **Forventet:**
      appen prøver å koble til igjen i en periode, og først når det
      forsøksbudsjettet er brukt opp, skal du se en ekte terminal feil (ikke
      en advarsel) — for tidlig, og en glemt kabel ser ufarlig ut for lenge;
      for sent, og en glemt kabel blir aldri oppdaget.
- [ ] **(c) `kill -9` midt i et planlagt opptak.** Legg inn et planlagt
      opptak («Ta opp automatisk» eller et spesialopptak) et par minutter
      fram i tid. Når det har startet, finn prosessen med `ps aux` og drep
      den hardt: `kill -9 <pid>`. Start appen på nytt.
      **Forventet:** en gjenopprettet fil dukker opp i **Redigering**s
      historikk ved neste oppstart — IKKE et «gikk glipp av»-varsel for det
      samme slotet. Et slot dekket av en gjenopprettingsrunde er ikke det
      samme som et slot ingen prøvde.
- [ ] **(c, fortsettelse) Slot og spesialopptak på samme tid.** Sett opp den
      faste ukentlige tiden OG et spesialopptak til å begynne i samme minutt.
      **Forventet:** appen starter ÉTT opptak, ikke to som kjemper om samme
      enhet.
- [ ] **(c, fortsettelse) Spesialopptak med eget lydkort.** La den vanlige
      lydenheten (Lyd-siden) være mikseren, og koble i tillegg til en
      USB-mikrofon. Gearikonet → Avansert → «Flere tider og spesialopptak»:
      legg inn et spesialopptak et par minutter fram, og velg USB-mikrofonen
      i den nye enhetsvelgeren (ikke «Samme som vanlig opptak»). Raden viser
      dato · tid · mikrofonens navn.
      **Forventet:** opptaket starter av seg selv og tar opp fra
      USB-mikrofonen (VU-en følger mikrofonen, ikke mikseren), med
      kanalparet Lyd-siden har for DEN enheten (standard 1/2 hvis du aldri
      valgte et) — ikke mikserens par. Neste ukentlige opptak tar fortsatt
      opp fra mikseren.
      **Deretter — trekk den ut:** legg inn et nytt spesialopptak på
      USB-mikrofonen, og trekk ut mikrofonen før det starter.
      **Forventet:** ved start tar appen opp fra mikseren — opptaket GÅR —
      og et systemvarsel sier «Lydenheten «‹navn›» for spesialopptaket var
      ikke tilgjengelig — opptaket bruker den vanlige lydenheten i stedet.»
      Varselet kommer også med «Varsle når opptak starter» slått av. Raden
      i lista viser «(ikke tilkoblet nå)» mens mikrofonen er ute. (Legger du
      det inn mer enn 30 min fram, skal også sjekken før opptak — en
      halvtime før start — si at lydenheten med mikrofonens navn ikke er
      tilkoblet.)
      **På Windows-boksen i tillegg** (enhetslista leses inne i den
      planlagte starten, med 5 s tak): 1) Med en ASIO-driver installert og
      en WASAPI-mikrofon valgt på spesialopptaket skal ingen
      ASIO-driverpanel poppe opp ved start, heller ikke når mikrofonen er
      trukket ut (samme regel som w14). 2) Med en ASIO-enhet valgt på
      spesialopptaket starter det på ASIO (diagnose:
      `set_audio_engine: asio`) — og loggen har INGEN linje «own audio
      device is unavailable» med `reason=Timeout` (det ville betydd at
      ASIO-sveipet tok over 5 s, og at opptaket falt tilbake til mikseren). 3) Ingen krasj i den planlagte starten (enumereringen går gjennom
      COM-ankeret, #282). 4) **ASIO til ASIO:** la den vanlige lydenheten
      være et ASIO-grensesnitt, og legg spesialopptaket på et ANNET
      ASIO-grensesnitt. Ha Opptak-siden åpen med VU-en i gang (eller
      forhåndsopptaket på) når det starter. **Forventet:** asio-sys laster
      bare ÉN ASIO-driver per prosess, så det andre grensesnittet ser ut som
      borte mens det første holdes — opptaket tar opp fra det VANLIGE
      ASIO-grensesnittet, og varselet om at spesialopptakets lydenhet ikke
      var tilgjengelig kommer. Sjekken før opptak (en halvtime før, mens VU
      eller forhåndsopptak holder driveren) skal derimot IKKE si at
      spesialopptakets enhet mangler — den sjekker den vanlige enheten,
      fordi «mangler» ikke kan fastslås da. Noter om varselet kom, og om
      opptaket faktisk gikk på det vanlige grensesnittet.
- [ ] **(d) WAL-sjekk på en ekte database.** Kjør appen mot en KOPI av en
      ekte `sundayrec.sqlite` (ikke en tom testdatabase) og ta opp normalt.
      **Forventet:** `sundayrec.sqlite-wal` og `sundayrec.sqlite-shm` finnes
      ved siden av hoveddatabasen mens appen kjører, og hele den eksisterende
      opptakshistorikken er intakt og lesbar i **Redigering** etterpå — WAL
      har ikke mistet noe som lå der fra før.
- [ ] **(e) Vekketest.** Skru på «Vekk maskinen fra dvale» (gearikonet →
      Avansert), og bruk **«Test vekking om 2 min»** på kortet «Flere tider
      og spesialopptak». La maskinen sovne (eller sovne den selv). Rett
      etter at testen har løst ut (eller du har trykket «Avbryt»), kjør
      `pmset -g sched` i Terminal.
      **Forventet FØR fiksen:** «Test vekking»/«Avbryt» gikk rett på pmset og
      **erstattet** hele vekkeplanen under samme eier (`SundayRec`) — en
      test lørdag kunne slette søndagens ekte vekking uten varsel, og
      heltekortet fortsatte å vise «armert» etterpå fordi det leser appens
      egen forventning, ikke OS-ets svar.
      **Forventet ETTER (F2-W3, #235):** maskinen våkner av seg selv rundt to
      minutter senere, uten et administratorpassord-spørsmål (med mindre
      appen selv har advart om at akkurat denne maskinen trenger et) — OG
      `pmset -g sched` lister søndagens vekking under eieren `SundayRec`
      **både før og etter** testen. Testens egen oppføring bruker en egen
      eier (`SundayRec-test`) og er borte etter «Avbryt»; søndagens er
      urørt.
- [ ] **(e, fortsettelse) Er `cancelall` trygg for eieren den ber om?** På
      samme rigg: `pmset schedule wake "<en dato/klokke 2 min fram>" Test`,
      deretter `pmset schedule cancelall SundayRec`, og les `pmset -g sched`
      mellom hvert steg.
      **Ubevist (#235s «utenfor scope»):** `man pmset` sier eieren er en
      valgfri hale til `type date+time`, men ingen har fått bekreftet på en
      ekte Mac om `cancelall SundayRec` faktisk filtrerer på eier (kun
      `SundayRec`-oppføringer forsvinner), kansellerer ALT uansett eier (også
      testens `Test`-oppføring), eller feiler stille. Svaret avgjør om den
      gamle, delte-eier-modellen noensinne var trygg på denne maskinen.
      Noter resultatet i `docs/NEEDS-RICHARD.md`.
- [ ] **(e, varsling — runde 3) Et tapt opptak sier fra tre steder.** Med
      «Vekk maskinen fra dvale» PÅ: legg inn en slot 5 minutter fram, avslutt
      SundayRec, og start den igjen godt over en time etter slot-starten.
      **Forventet ETTER (runde 3, `docs/VARSLING.md`):** (1) ett systemvarsel
      med slot-navnet og tiden på appens språk («Planlagt opptak ble ikke
      gjort: Ukentlig opptak (…) (søn. 11:00).», ikke en ISO-streng); bytt
      språk og start på nytt — varselet skal IKKE komme igjen. (2) Menylinja
      har gult merke og statusraden «Planlagt opptak ble ikke tatt — klikk for
      detaljer» til neste opptak starter. (3) Avansert → Vekkehistorikk har én
      linje «<dag dato klokke> — Gikk glipp av en planlagt vekking». Gjenta med
      vekking AV: varsel og menylinje som over, men ingen ny linje i
      vekkehistorikken.
- [ ] **(g) #111 — lyttetest med ulik inngangsgain.** Ta opp 3–4 korte klipp
      av den samme typen lyd (tale er nok) med tydelig ulik inngangsgain —
      stille, normal, kraftig. Lytt gjennom dem, og se spesielt etter om tale
      blir feilklassifisert (kuttet bort, eller behandlet som musikk/stillhet)
      ved de mest ekstreme nivåene.
      **Forventet:** alle fire oppfattes riktig som tale uansett gain-nivå —
      dette er E10-regelen: `SPEECH_FLUX_MIN` er skala-avhengig (samme tale
      10 dB varmere gir omtrent 3× flux), og bare et ekte øre på ekte
      opptak kan bekrefte at terskelen ikke er kalibrert for kun ett
      lydnivå. Noter resultatet i #111 uansett utfall.
- [ ] **(tillegg — #252, macOS-siden av holde-våken-blokken) Holder maskinen
      seg våken før et planlagt opptak?** Sett «Sove etter» kort
      (Systeminnstillinger → Batteri/Strømadapter → 1 min), legg inn en slot
      15 minutter fram, og la maskinen stå.
      **Forventet FØR fiksen:** avgjørelsen om å holde maskinen våken var
      portet fra Electron (`wake.rs`s `should_block`), men HANDLINGEN var
      det aldri — ingen `IOPMAssertionCreateWithName`-kall fantes. En maskin
      vekket av `pmset` ti minutter før gudstjenesten kunne sovne igjen på
      den vanlige idle-timeren før opptaket i det hele tatt startet.
      **Forventet ETTER (F2-W5, #252):** maskinen står våken forbi
      1-minutts-grensa når klokka passerer T−30 min, og opptaket starter i
      tide. Kjør `pmset -g assertions` i vinduet: den skal liste eieren
      `SundayRec` med grunnen «scheduled recording is due», og under selve
      opptaket samme eier med grunnen «recording in progress» — ingen av dem
      igjen etterpå. Ta med i samme runde: skjermen skal FORTSATT få sove
      (blokken ber med vilje ikke om en skjerm-holdt-våken-variant), og sjekk
      om oppførselen er den samme på batteri som på nettstrøm (respekten for
      `PreventSystemSleep` er ikke garantert lik på batteri).
- [ ] **(h) Et helt ekte 90-minutters opptak.** Skru på «Del opp lange
      opptak» med en kort grense (f.eks. 30 min) og ta opp en hel ekte
      gudstjeneste eller tilsvarende lengde med tale.
      **Forventet:** opptaket deles i flere filer ved de riktige
      intervallene, ingen fil er tom eller korrupt, og loggen roterer som
      forventet uten å miste noe — bekreft at de roterte filene
      (`sundayrec.1.log` … `.4.log`) er intakte og i riktig rekkefølge
      etterpå. Dette er også den økten som gir de ærlige tallene til
      RELEASE-CHECKLIST.md §6a (Dropp/xruns/IPC-overbelastning) hvis noe i
      opptaksmotoren er endret siden sist.
- [ ] **(e1) Manuelt opptak fra Opptak-sida lagres som før (navn/mappe).**
      Noter opptaksmappa (mappe-kortet) og filnavnmønsteret, trykk «Start
      opptak» på Opptak-sida, ta opp ~20 s og stopp. Gjenta én gang med
      videobryteren på og et kamera valgt.
      **Forventet FØR fiksen:** fila havnet i opptaksmappa med navnet
      mønsteret gir — men stien kom fra nettvisningen, og en kompromittert
      side kunne pekt den hvor som helst.
      **Forventet ETTER (sikkerhetsfunn E1):** nøyaktig samme mappe og samme
      navn som før (`_2` på et nytt opptak samme dag), og `.mp4` i samme
      mappe for videoopptaket; «Vis i Finder» på kvitteringen viser fila.
      Utstien planlegges nå i Rust — et avvik her er et funn mot E1, ikke mot
      opptaksmotoren.

- [ ] **(e2) Langt videoopptak på Mac under CPU-last (`-realtime 1`).**
      Slå på videobryteren med et kamera valgt (maskinvareenkoder på, som
      standard), start et opptak, og belast CPU-en samtidig, for eksempel med
      `yes > /dev/null` i fire til åtte Terminal-vinduer eller en tung
      eksport i Redigering. La det gå **minst 60 minutter** (helst en hel
      gudstjeneste). Stopp, og spill av fila.
      **Forventet:** ingen manglende bilder eller frysninger, og bilde og lyd
      i takt helt til slutten (klapp i begynnelsen og slutten, og se at lyden
      treffer). Opptaksoverlegget viser ikke at enkoderen sakker akterut.
      Eierbeslutning 2026-10-04: robusthet foran kvalitet, så litt mykere
      bilde ved lav bitrate er riktig pris, men tapte bilder eller drift er
      et funn. Denne sjekken gjelder opptaket; en eksport i Redigering har
      ikke flagget og skal være like skarp som før.

## Windows-boksen

- [ ] **(b) Kamera + video, stopp og start rett etter hverandre.** Ta opp en
      videoøkt (kamera + lyd), stopp den, og **innen få sekunder** start et
      helt nytt opptak (f.eks. et kveldsmøte rett etter gudstjenesten).
      **Forventet:** det nye opptaket viser en ren «Tar opp»-tilstand — ingen
      rest av forrige økts «Stoppet»-tilstand vises over det nye. En
      generasjon som ikke er den gjeldende skal ikke få lov til å skrive til
      skjermen.
- [ ] **(f) ASIO-delmengde.** Fra `docs/ASIO-TEST-MATRIX.md`, kjør minst:
      byggsjekken (`asio_spike`-eksempelet lister en enhet), én vanlig
      WASAPI-opptak (USB-mikrofon eller lydkort), én ASIO-opptak på et
      pro-lydkort med kanalvalg, og USB-uttrekk midt i et ASIO-opptak
      (skal finalisere pent med «device_disconnected», ikke henge).
      **Forventet:** alle fire består som beskrevet i den fulle matrisen —
      dette er ikke en erstatning for den, bare det minste utvalget som hører
      hjemme på en dag som ellers handler om Mac-boksen.
- [ ] **(w1) Windows-oppdatering installerer seg selv.** Fra en installert
      NSIS-beta på Windows, med Oppgavebehandling åpen: gearikonet →
      Avansert (eller banneret) → «Se etter oppdateringer» → «Last ned og
      installer».
      **Forventet FØR fiksen:** vinduet forsvant, ingen installer-dialog kom,
      og appen startet ikke på nytt — neste gang appen ble åpnet, var det
      fortsatt den gamle versjonen, uten en eneste feilmelding eller
      logglinje (årsak: appens eget ffmpeg-jobbvern la installeren i samme
      Job Object som SundayRec selv, og oppdateringspluginens `exit(0)` etter
      utpakking tok installeren med seg i fallet).
      **Forventet ETTER (F2-W1, #243):** en synlig, passiv installer kjører,
      `SundayRec_*_x64-setup.exe` blir IKKE drept i Oppgavebehandling, og
      appen kommer tilbake i den nye versjonen. `update-relaunch.log`
      (i appdata-mappa: `%LOCALAPPDATA%` fra og med F-W10, `%APPDATA%` for
      versjoner før) skal ha en linje som starter med `installing` og
      slutter med `kill-on-close disarmed: true`.
- [ ] **(w2) Oppdatering er sperret mens det tas opp.** Start et opptak, gå
      til banneret / gearikonet → Avansert.
      **Forventet FØR fiksen:** «Last ned og installer» var trykkbar midt i
      et opptak, og ett klikk kjedet rett videre til omstart — ingen
      advarsel om at et opptak gikk.
      **Forventet ETTER (F2-W1, #243):** knappen er grå med forklaringen
      «Kan ikke oppdatere mens det tas opp», både i banneret og under
      Avansert. Stopp opptaket → knappen virker igjen med én gang.
- [ ] **(w3) Ingen svarte konsollvinduer.** Gjennom én økt på Windows, se
      etter et svart/konsoll-vindu ved hvert av disse seks stegene: 1) start
      appen og enhetslisten, 2) start et lydopptak, 3) start et opptak MED
      video, 4) stopp opptaket (leverings-transkodingen), 5) last et opptak i
      Redigering og kjør en eksport, 6) bruk «Test vekking».
      **Forventet FØR fiksen:** et svart konsollvindu ved hver enhetsliste og
      hver start; ett stående i minuttvis under leverings-transkodingen; med
      video, ett stående HELE gudstjenesten — og lukker en frivillig det ved
      et uhell, dør opptaket (ffmpeg mottar `CTRL_CLOSE_EVENT`).
      **Forventet ETTER (F2-W2, #237):** ingen av de seks stegene viser noe
      konsollvindu, noen gang.
- [ ] **(w5) Samme holde-våken-blokk, Windows-siden.** Sett «Sove etter» til
      **1 minutt** (Innstillinger → System → Strøm), legg inn en slot
      **15 minutter fram**, og la maskinen stå.
      **Forventet FØR fiksen:** samme hull som på Mac-en (se Mac-boksens
      tillegg — #252): planleggerens `SetWaitableTimer` vekker maskinen ti
      minutter før start, men Windows' «System unattended sleep timeout»
      (standard 2 min) tar den tilbake til dvale før noe har åpnet en power
      request — ingen `SetThreadExecutionState`-kall fantes.
      **Forventet ETTER (F2-W5, #252):** maskinen står våken forbi
      1-minutts-grensa når klokka passerer T−30 min, og opptaket starter.
      `powercfg /requests` skal vise **SundayRec** under `SYSTEM` i
      mellomtiden — og IKKE 40 minutter før start. Skjermen skal fortsatt få
      sove (blokken bruker `ES_SYSTEM_REQUIRED`, ikke `ES_DISPLAY_REQUIRED`).
- [ ] **(w6) Drep appen midt i et videoopptak.** Start et videoopptak
      (kamera + lyd), drep prosessen i Oppgavebehandling midt i økten, start
      appen på nytt.
      **Forventet FØR fiksen:** cpal-videostien på Windows skrev rett til
      brukerens `.mp4` uten noe gjenopprettingsmanifest — en krasj (eller et
      strømbrudd, en omstart fra Windows Update, eller det lukkbare
      konsollvinduet fra (w3)) ga en UAVSPILLBAR fil, og appen visste ikke
      ved neste oppstart at opptaket i det hele tatt hadde eksistert.
      Gudstjenesten var borte, uten et ord.
      **Forventet ETTER (F2-W4, #246):** en SPILLBAR fil dukker opp i
      Redigerings historikk ved neste oppstart, merket «Gjenopprettet etter
      uventet avslutning», og den skjulte `.sundayrec-capture-*`-mappa er
      borte etterpå.
- [ ] **(w6, fortsettelse) Ta med i samme runde:** (a) et helt normalt
      videoopptak stoppet pent — filen skal ligge som vanlig mp4, og den
      skjulte mappa skal være ryddet; (b) lepp-synk over en hel gudstjeneste
      på denne stien er fortsatt HARDWARE-UVERIFISERT (uendret av #246 — det
      er bare krasjsikkerheten som er ny, ikke synk-kvaliteten).
- [ ] **(tillegg — #235, Windows-siden av vekketesten).** Sett en ekte
      ukentlig vekketid noen minutter fram, bruk «Test vekking om 2 min», la
      testen løse ut eller trykk «Avbryt», og vent til den EKTE planlagte
      tiden.
      **Forventet FØR fiksen:** testens `timers.clear()` lukket ALLE
      `SetWaitableTimer`-håndtak samtidig — søndagens vekking kunne dø
      sammen med testens egen.
      **Forventet ETTER (F2-W3, #235):** testen og den ekte planen ligger i
      hvert sitt `TimerSlot` (Schedule/Test); maskinen skal våkne på den ekte
      tiden uansett hvor mange ganger testknappen er brukt i mellomtiden. 👤
      Windows har ingen `pmset -g sched`-motsvarighet for å inspisere en
      armert timer utenfra — beviset her er rent behaviorelt (våkner den,
      eller gjør den ikke).
- [ ] **(tillegg — #231) Fire lydtester som bare kan klassifiseres på ekte
      Windows-maskinvare.** Kjør `cargo test --workspace` i `src-tauri` på
      selve riggen (ikke CI), med en ekte mikrofon tilkoblet:
      `audio::vu::tests::vu_stream_negotiates_max_channels_or_skips`,
      `recorder::native_capture::segment::tests::native_capture_records_two_seconds_or_skips`,
      `…preroll::tests::native_preroll_buffers_meters_and_harvests_or_skips`,
      `…segment::tests::a_failed_spawn_leaves_no_capture_file`.
      **Forventet i CI (uten rigg):** alle fire er
      `#[cfg_attr(windows, ignore = "F2-W7: … — se PR #231")]` — CI-runnerens
      image krasjer (`STATUS_ACCESS_VIOLATION`) så snart de faktisk BYGGER en
      cpal-strøm (`IAudioClient::Initialize`), etter alt å dømme fordi
      runner-imaget mangler en fungerende Windows-lydtjeneste.
      **Forventet på riggen:** alle fire kjører og består — ikke bare hopper
      over. Består de ikke her heller, er det et ekte funn, ikke et
      rigg-artefakt; skriv det opp mot #231.
- [ ] **(w14) Unødvendig ASIO-sveip ved vanlig opptak.** Windows-maskin med
      **én ASIO-driver installert** (ASIO4ALL holder) og en **WASAPI-enhet
      valgt** i SundayRec.
      **Forventet FØR fiksen:** hver opptaksstart, hver enhetsliste-åpning og
      kapasitetsbenken kalte `is_asio_device` ubetinget — det sveipet ALLE
      installerte ASIO-drivere (`host.devices()` → `ASIOInit` per driver) selv
      når en helt vanlig WASAPI-enhet var valgt. Kunne poppe et driverpanel
      opp midt i opptaksstart, eller ta lydkortet et ASIO-opptak skulle bruke
      to linjer senere — verst på et planlagt opptak ingen står ved.
      **Forventet ETTER (F2-W8, #253):** fire ting, i samme økt. 1) Ta tiden
      fra klikk til at teller/VU løper, og sammenlign med v0.18.0-beta.1 —
      startforsinkelsen skal være borte. 2) Driverpanelet popper ikke opp ved
      opptaksstart lenger. 3) ASIO-stien er fortsatt intakt: velg
      ASIO-enheten i velgeren og trykk opptak innen 30 s — diagnose viser
      `set_audio_engine: asio`, og alle kanalene er der. 4) Ferskhet: plugg
      inn et ASIO-grensesnitt mens appen står åpen, vent over 30 s, åpne
      velgeren igjen — det skal dukke opp; kjør diagnose og den ser det med
      én gang (den dropper 30 s-memoet først).

- [ ] **(we1) Det samme på Windows: manuelt opptak lagres som før
      (navn/mappe).** Som (e1) på Mac-boksen, med en WASAPI-enhet og — har
      riggen en — én gang med en ASIO-enhet.
      **Forventet ETTER (sikkerhetsfunn E1):** fila i opptaksmappa (standard
      `Dokumenter\SundayRec`) med samme navn som før, `_2` på et nytt opptak
      samme dag; «Vis i Utforsker» på kvitteringen viser den.

- [ ] **(w-appdata) Databasen og appdataen flytter fra Roaming til Local
      (F-W10, avgjort 2026-10-04).** Oppgrader en installasjon som HAR
      historikk (v0.25.0 eller eldre med minst noen opptak og endrede
      innstillinger) til denne versjonen. Gjør det slik at `sundayrec.sqlite`
      har et ikke-tomt `-wal` (la appen stå en stund etter siste opptak, og
      avslutt den med Oppgavebehandling i stedet for «Avslutt» før
      oppgraderingen).
      **Forventet:** (1) første start viser all historikken og alle
      innstillingene (språk, opptaksmappe, planlagte slots); (2)
      `%LOCALAPPDATA%\no.sundayrec.app\sundayrec.sqlite` finnes, ingen
      `sundayrec.sqlite.flytter` blir liggende; (3) den gamle
      `%APPDATA%\no.sundayrec.app\sundayrec.sqlite` finnes fortsatt, urørt;
      (4) loggen (`%LOCALAPPDATA%\…\logs\sundayrec.log`) har en linje
      «the database was moved from Roaming to Local AppData»; (5) andre start
      flytter ikke igjen og viser ingen banner. **Nedgradering:** installer
      v0.25.0 over den; den skal åpne den gamle databasen med historikken
      (ikke en tom).
- [ ] **(w-appdata-recovery) Krasj → oppdatering → gjenoppretting.** På en
      v0.25.0-installasjon: start et opptak, drep appen med
      Oppgavebehandling midt i (manifestet ligger nå i
      `%APPDATA%\no.sundayrec.app\recovery`), oppgrader til denne
      versjonen, start den. **Forventet:** det avbrutte opptaket dukker opp
      i Historikk som gjenopprettet fil, og manifestet er borte fra
      `Roaming\…\recovery`. Gjør det en gang til MED SAMME manifest i begge
      mappene (kopier den ene filen til `Local\…\recovery` før start):
      opptaket gir fortsatt ÉN rad i Historikk, ikke to (duplikater fanges av
      `recording_exists_for_path`), og begge manifestene er borte etterpå.
- [ ] **(w-appdata-fallback) En flytting som feiler mister ingenting.**
      Lag `%LOCALAPPDATA%\no.sundayrec.app\sundayrec.sqlite.flytter` som en
      MAPPE (hindrer tempfila), start. **Forventet:** appen starter med
      historikken fra Roaming (ikke tom), loggen sier at flyttingen feilet,
      og ett banner kommer etter ca. 20 s med katalogteksten
      «Historikken og innstillingene ble ikke flyttet til den nye mappen …»
      (på valgt språk); neste start (uten hindringen) flytter.
- [ ] **(w-appdata-nedgradering) Data fra en nedgradert økt blir ikke borte
      i det stille.** Etter en vellykket flytting: installer v0.25.0 over, endre
      en innstilling og ta et testopptak, avslutt normalt, installer denne
      versjonen igjen. **Forventet:** appen viser den nye mappas historikk
      (ikke testopptaket), og ett banner etter ca. 20 s: «Historikk og
      innstillinger fra den eldre versjonen ligger igjen i den gamle mappen …».
      Samme start en gang til: ingen nytt banner.
- [ ] **(w-appdata-nett) Roaming som ikke er nådd er ikke en ny installasjon.**
      På en maskin med omdirigert Roaming-mappe: koble fra nettverket/VPN, start
      appen FØR Local har en database (f.eks. etter å ha flyttet
      `%LOCALAPPDATA%\no.sundayrec.app\sundayrec.sqlite` bort). **Forventet:**
      ingen ny database i Local; appen bruker Roaming slik den alltid har gjort
      (og feiler om den heller ikke kan nås), og første start MED nettverk
      flytter databasen.

- [ ] **(w7) Windows-installasjon: NSIS uten UAC, og MSI → NSIS.** Tre ting,
      på en Windows-maskin med en vanlig bruker (ikke administrator) — helst en
      ren maskin eller VM. Bevis fra release-bygget finnes ikke for noen av dem
      (se `docs/RELEASE-CHECKLIST.md` §5a); dette er den første kjøringen. 1) **Ny installasjon:** kjør `SundayRec_*_x64-setup.exe` som vanlig bruker.
      **Forventet:** ingen UAC-spørsmål; appen havner under
      `%LOCALAPPDATA%\SundayRec`, ikke `Program Files`. 2) **Uten WebView2:** på en maskin der WebView2 ikke finnes (eldre
      Windows 10 / ren VM), én gang MED og én gang UTEN nett. **Forventet:**
      med nett installeres WebView2 av den innebygde bootstrapperen uten at
      noe lastes ned først; uten nett ventes det å feile — `embedBootstrapper` trenger
      fortsatt nett for selve runtimen, og eier har valgt bort
      `offlineInstaller` (+127 MB på hver oppdatering; `NEEDS-RICHARD.md`).
      Bevis det, men en feil uten nett er ikke et funn. 3) **MSI → NSIS:** installer en eldre `.msi` (v0.25.0, som administrator),
      og la appen oppdatere seg til en utgivelse der `-msi`-nøkkelen er med:
      **Forventet:** `.msi` oppdaterer over seg selv (UAC som før), én
      installasjon. Test deretter, med `-msi`-nøkkelen fjernet fra en
      testfeed, at NSIS-stien avinstallerer MSI-en og ikke etterlater to
      installasjoner — og hva en ikke-administrator ser da.

_(w4, w8–w13, w15 hører til andre F2-Windows-funn som løper i egne
runder — skjulte mapper + OneDrive-varsel, m.fl. Fylles inn her når de
respektive PR-ene er merget; se `docs/NEEDS-RICHARD.md` §«Eierbeslutninger fra F2».)_

## Ørene

Lydkjede-funn som verken CI eller en sidecar-måling kan avgjøre alene — de
trenger et ekte øre på ekte opptaksmateriale (ikke bare et lavfi-testsignal).
Kjør disse på Mac- eller Windows-boksen, med hva du har av ekte
gudstjeneste-opptak eller -materiale for hånden.

- [ ] **(i–ii) Mastringen holder løftet — lineær, ikke gain-ridd.** Eksporter
      ÉN ekte gudstjeneste med presetet `speech-clear` fra denne grenen, og
      lytt etter **pumping** (nivået som kryper opp i pausene og ned igjen
      når stemmen kommer — det er gain-rideren). Sjekk deretter
      eksport-loggens `Normalization Type`-linje, og prøv en fil med harde
      topper (mikrofonhåndtering, en dør) for å se om kvitteringen sier
      «(begrenset av topper)».
      **Forventet FØR fiksen:** loudnorm-presetene lovet en «lineær»
      forsterkning, men pass 2 sendte presetets LRA/TP-tall rått uten å
      sjekke om lineær faktisk var oppnåelig — en preken med normalt
      dynamisk spenn falt nesten alltid tilbake til ffmpegs 3-sekunders
      gain-rider, stille, uten at kvitteringen sa noe om det.
      **Forventet ETTER (F2-C-B, #245):** nivået ligger stille, ingen
      pumping; loggens `Normalization Type` sier `Linear` for normalt
      materiale (sier den `Dynamic`, kommer en `warn!` ved siden av — da er
      MODELLEN feil, ikke bare uflaks); en fil med harde topper får en
      lavere, ærlig rapportert LUFS med «(begrenset av topper)» i
      kvitteringen i stedet for at loudnorm komprimerer den ned til målet.
- [ ] **(iii) Mikser-default etter dB-fiksen.** Rediger → «Avansert: åpne
      mikseren» på et ekte prekenopptak, la standardverdiene stå (kompressor
      på, terskel −18 dB, makeup 2 dB), eksporter, og les integrert loudness + true peak mot en eksport gjort med v0.17.x. Ta med en runde på
      gate-slideren i bunn (−70 dB).
      **Forventet FØR fiksen:** makeup/limiter/gate ble tolket LINEÆRT i
      stedet for i dB — 2 dB makeup ga i praksis +6 dB, en 0 dBTP-takgrense
      slapp gjennom med +1 dB på kjøpet, og gate-slideren i bunn (−70 dB) var
      i praksis helt åpen (stengte aldri, for noe).
      **Forventet ETTER (F2-C-A, #238):** samme materiale gir omtrent 4 dB
      mindre gain inn i loudnorm, true peak lander på −1,0 dBTP der den før
      lå på 0,0, og gate-slideren i bunn stenger nå faktisk mellom
      setningene.
- [ ] **(iv) Knappetrykket midt i en akkord.** Trykk opptak midt i musikk —
      prøv én gang med noe stille (lettest å høre et klikk), én gang med
      orgel på full styrke (der marginen kostet mest).
      **Forventet FØR fiksen:** forhåndsbufferen kastet alltid de siste
      300 ms før knappetrykket (en ffmpeg-motor-sikkerhetsmargin arvet
      ubetinget av den native lydstien), og skjøten mot selve opptaket var
      et loddrett PCM-sprang — hørbart som et klikk.
      **Forventet ETTER (F2-C-D, #247):** hullet foran knappetrykket er
      omtrent 300 ms kortere (nesten hele forhåndsbufferen er med), og
      skjøten klikker ikke — klippet rampes ned til digital null de siste
      10 ms.

**Ikke en egen sjekk lenger:** kanaldiagnosen (dødt/knitrende kabel,
kanal-duplisering) var tidligere en lytte-sjekk, men er nå fullt
mutasjonstestet mot ekte ffmpeg-sidecar-målinger på kjente L/R-nivåer
(F2-C-C, #248) — se PR-teksten for måletabellen. Den trenger ikke et øre på
riggdagen lenger.

_(v–ix hører til resten av lydkjede-gjennomgangen (rapport C) — ingen av dem
endte som en kodefiks i den runden. De ble eierspørsmål, og står i
`docs/NEEDS-RICHARD.md` §«Eierbeslutninger fra F2» merket «(C, mening)».
Eier avgjorde 2026-10-04: `-realtime 1` for VideoToolbox-opptaket er
gjennomført (se (e2) i Mac-boksen over), mens den egne «Kirke»-mastringsprofilen
og automatisk monolevering venter på en lyttetest. De to siste har ingenting
å rigg-teste før en PR har landet.)_

## Etterpå

- [ ] Oppdater `docs/NEEDS-RICHARD.md`s HARDWARE-UNVERIFIED-liste: fjern det
      som nettopp ble bevist, eller noter et nytt funn mot punktet det hører
      til.
      **Forventet:** bare de reelt fortsatt-uverifiserte tingene er igjen.
      Ikke slett hele lista fordi mesteparten av dagen gikk bra — én dag
      dekker ikke alt.
- [ ] Kryss av tilhørende bokser i `docs/SMOKE-TEST.md` og
      `docs/ASIO-TEST-MATRIX.md` der de overlapper med det du nettopp kjørte.
- [ ] Er alt grønt, og en utgivelse venter på nettopp denne dagen: fortsett
      til `docs/RELEASE-CHECKLIST.md` §6.
