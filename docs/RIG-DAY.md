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
- [ ] **(a, A3b) Bytt samplingsfrekvens midt i opptaket.** Start et opptak
      på mikseren (eller innebygd mikrofon), la det gå et minutt, og bytt
      enhetens frekvens i **Audio MIDI-oppsett** (f.eks. 48 000 → 44 100 Hz).
      **Forventet:** én gjenkoblingsstripe (advarsel `device_disconnected`,
      ingen feilmelding), og opptaket fortsetter. Frekvensen du valgte i
      Audio MIDI-oppsett **blir stående** — appen setter den ikke tilbake.
      Etterpå er det TO filer (ny levering ved ny frekvens, ikke en skjøtet
      fil med to frekvenser), og loggen har linjene «stream invalidated» og
      «device rate changed across reconnect» etter hverandre. **Rødt funn:** frekvensen
      hopper tilbake av seg selv, eller gjenkoblingen gjentar seg i en løkke.
      Klassifiseringen er bevist på Macen med
      `cargo test -p sundayrec rate_change_under_a_running_stream -- --ignored`
      (ekte CoreAudio, 2026-10-12); dette punktet beviser resten av veien.
- [ ] **(a, A3b) Lydformatet i Audio MIDI-oppsett før og etter opptak.** Noter
      enhetens format (f.eks. «2 kan. 24-bit heltall 48,0 kHz») før du starter,
      start et opptak, og se igjen. cpal 0.18 prøver nå å sette enhetens
      FYSISKE format (bitdybde) til det appen ber om. **Forventet:** rate
      uendret, og bitdybden enten uendret eller endret uten at andre programmer
      på mikseren merker det. Endres den, noter det — det er et funn å vurdere,
      ikke nødvendigvis en feil.
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
      etterpå. **Xruns (A3c):** diagnoserapporten skal vise antall xruns for
      opptaket. Det er tallet lydsystemet selv melder (CoreAudio/WASAPI via
      cpal), ikke noe lest ut av en logg. Noter det per maskin: 0 på en
      rolig maskin er det vanlige, og et tall over 0 skal stemme med
      hakk du faktisk kan høre eller se i bølgeformen. Dette er også den økten som gir de ærlige tallene til
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

- [ ] **(a1b) Lisensfila ligger i den installerte appen.** Installer en ekte
      utgivelse (ikke en `tauri dev`-bygging). Gearikonet → Avansert → bunnen:
      kortet «Lisenser» → «Vis lisenser».
      **Forventet:** en side åpner seg i appen, med «FFmpeg (GPL)» øverst og
      lenker til hvor kildene finnes, så npm- og Rust-bibliotekene med
      lisenstekst. Knappen gir ikke feilmeldingen «Fant ikke lisensteksten».
      Se også at fila finnes: macOS `SundayRec.app/Contents/Resources/resources/THIRD-PARTY-NOTICES.html`,
      Windows `resources\THIRD-PARTY-NOTICES.html` ved siden av `SundayRec.exe`.
      Gjør det på begge maskinene. (Macen kan også gjøres uten rigg, men
      installert utgivelse + signert bygg er det som beviser at bunten er riktig.)

- [ ] **(c1a) Native kamerafangst på Macen: FaceTime HD og Continuity (C1a,
      `ombygging/c1a-kamera-mac`).** AVFoundation-porten bygger og
      enhetstestene er grønne, men den er IKKE kjørt mot et kamera: økta gikk
      uten skjerm, terminalen hadde ikke kameratilgang (`NotDetermined`), og
      tillatelsen ble ikke omgått — maskintesten fikk null bilder. Dette punktet
      er beviset. Trenger ikke riggen, bare Macen og iPhonen. **Gjør:**
      (1) Gi terminalen du kjører fra kameratilgang (Systeminnstillinger →
      Personvern og sikkerhet → Kamera; eller svar ja på spørsmålet).
      (2) Legg iPhonen låst i nærheten, samme Apple-ID, Wi-Fi og Bluetooth på.
      (3) Kjør fra repoet:
      `cargo test -p sundayrec --lib camera::avfoundation -- --ignored --nocapture`.
      Bare ett kamera: `SUNDAYREC_CAMERA=iphone` (eller `facetime`) foran.
      **Forventet:** for HVERT kamera en blokk med `mode Native`,
      `delivered` > 100 på 5 s, `without_image: 0`, en latens
      «pts→callback» mellom 0 og noen titalls ms (fiks 7: tidsstempelet er
      på vertsklokka), og målt fps **ikke over** valgt fps (fiks 2: ikke
      «strukket»). iPhonen skal stå som `Continuity`, FaceTime som `BuiltIn`.
      **Rødt funn:** `mode Compatibility` (den native økta leverte ikke —
      fiks 3 slo inn; noter kamera og format), fps over valgt, eller
      testen henger i stedet for å feile. USB-kamera: gjenta på riggdagen med
      et USB-kamera tilkoblet; det skal stå som `External` med `usb`
      leverandør:produkt — det samme paret også etter bytte av USB-port.

- [ ] **(c1c-m) `camera_probe` på Macen: opptak til rå fil, med statistikk
      (C1c, `ombygging/c1c-kamera-api`).** Lista og formatene er kjørt
      (FaceTime HD, iPhone og skrivebordsvisning), men selve opptaket er IKKE:
      terminalen hadde ikke kameratilgang (`NotDetermined`, startforsøket ga
      `NotAuthorized` etter 5 s) og tillatelsen ble ikke omgått. Gjør først
      (c1a) over, så tilgangen finnes. **Gjør:** fra repoet
      `cargo run --example camera_probe -- --camera facetime --seconds 10`, og
      samme med `--camera iphone`. **Forventet:** per kamera «backend:
      AVFoundation, Native mode», første bilde innen få sekunder (iPhonen
      kan trenge 1–2 s), «measured fps» ved valgt fps (30), jitter under noen
      få ms, «lost» 0 og «dropped» 0/0/0 (kø/OS/hoppet over). **Rødt funn:**
      «Compatibility mode», «NEVER», tapte bilder eller kø-drop på et ellers
      stille system, tidsstempler som går bakover (`NON-MONOTONIC`).
      **USB-kamera (riggdagen):** `--list` skal vise det som «kamera» med
      `model` (vendor:product) som er lik etter bytte av USB-port; deretter
      samme 10 s-opptak mot det. Prøv også `--format 1920x1080@30` på alle tre.
      **Opptakskort finnes ikke** (hverken her eller på Windows-boksen): at et
      kort står som «opptakskort» og leverer bilder er IKKE bevist. Føres som
      åpent, ikke blokkerende.
      **Kjørt på Macen 2026-10-14 (eierens terminal, med kameratilgang):**
      FaceTime HD 1280x720 NV12 — Native mode, første bilde 0 ms, 302 bilder,
      29,51 b/s, jitter 4,1 ms, 4 hull fra kameraet (maks 74 ms), dropp 0/0/0.
      iPhone (Continuity) 1280x720 NV12 — Native mode, 309 bilder, 29,90 b/s,
      jitter 4,3 ms, 2 hull (maks 100 ms), dropp 0/0/0. Hullene er kameraets
      egne (lys), ikke våre; konformeren (C2) gjentar forrige bilde. Mac-delen
      er dermed grønn; USB-kamera og `--format 1920x1080@30` gjenstår.

- [ ] **(c3a) Den native videoøkta med ekte kamera, og `kill -9` midt i (C3a, `ombygging/c3a-videospor`).** Bevist i C3a med SYNTETISK bilde og lyd (WAV + MKV per fragment, én mp4, manifestet lister begge, `kill -9` av appen — og av appen OG koderen — gjenopprettet til mp4 med hele lyden, dropp-telleren øker når køen tvinges full). IKKE bevist: ekte kamera — terminalen hadde fortsatt ikke kameratilgang (`NotDetermined`) da økta ble kjørt. Gjør (c1a) først. **Gjør 1 (kamera, syntetisk lyd):** kjør `SUNDAYREC_REAL_CAMERA=FaceTime cargo test -p sundayrec --lib real_camera_killed -- --ignored --nocapture` fra repoet. **Forventet:** testen går grønt og skriver `kill -9 … wav ~4 s → mp4 video ~3,5 s, audio ~4 s` (videoen slutter inntil ≈0,5 s før lyden: konformerens beslutningsvindu). **Gjør 2 (hele appen):** start appen (siden C3b går alle videoopptak gjennom den native motoren — bryteren `SUNDAYREC_NATIVE_VIDEO` er borte), ta opp 2 min med FaceTime-kameraet og ekte mikrofon, klapp i starten og slutten, stopp. **Forventet:** én mp4 der klappet er synkront (±1 bilde) i begge ender, og en egen lydfil hvis «behold ren lydfil» er på. Med pre-roll på: de første sekundene har lyd og svart bilde i QuickTime, VLC og redigereren (mp4-en starter med en tom redigering, ikke kodet svart — se VIDEOMOTOR §C3 «Levert (C3a)»). Gjenta, men `kill -9` appen etter 1 min og start den igjen: opptaket kommer tilbake som mp4 med bilde og hele lyden. **Rødt funn:** bildet driver fra lyden, svart bilde med et kamera som virker, lyd som mangler, eller en opptaksøkt som henger på «Stopper».

- [ ] **(c3b) Ett videoopptak uten de gamle stiene (C3b, `ombygging/c3b-slett-gamle-stier`).** C3b slettet ffmpeg-fangsten; alt under er bevist i testene bortsett fra det som trenger et ekte kamera. **Gjør:** på Opptak, slå på «Ta med kamera» og velg FaceTime-kameraet. **Forventet:** kameralista viser de samme navnene som før (den leses nå fra den native kameralista), og kortet sier hva kameraet leverer («maks 1080p · 30 bilder i sekundet» e.l.) — ikke «fikk ikke spurt». Ta opp 1 min med klapp i start og slutt. **Forventet:** overlegget viser et levende bilde mens det tar opp (forhåndsvisnings-JPEG-en skrives nå av videosporets ffmpeg), én mp4 med bilde og synkront klapp, og en egen lydfil hvis «behold ren lydfil» er på. Kjør Diagnose etterpå: «Video» er testet og OK. Velg så et kamera som ikke er koblet til (trekk ut iPhone-en etter at du valgte den) og ta opp 20 s. **Forventet:** opptaket går som rent lydopptak (loggen sier «no camera matches the stored name — recording the sound only»), ikke svart video. **Rødt funn:** ingen bilde i overlegget med et kamera som virker, «fikk ikke spurt» om et kamera som virker, svart video der kameraet mangler, eller at opptaket ikke starter.

## Windows-boksen

- [ ] **(c3b-win-sync) Bildets fangsttid på Windows (M3R #5).** Før var bildene stemplet ved _ankomst_; nå brukes kameraets samplingstid (`FrameStamper`). **Gjør:** USB-webkamera (helst også et opptakskort/HDMI og et virtuelt kamera), start et opptak med loggen åpen (`%LOCALAPPDATA%\…\logs\sundayrec.log`) og finn `camera: frame time source` (`SampleTime`, `Anchored` eller `Arrival`, og `late_ms`). Ta opp 2 min med skarpt klapp (hendene foran kameraet, mikrofonen ved siden) i start og slutt; åpne mp4-en bilde for bilde og mål avstanden fra klappets lydtopp til bildet der hendene møtes. Gjør det samme på en bygning FØR fiksen (v0.26.0-beta.1) hvis du har en. **Forventet:** forskyvning innenfor ±1 bilde (≤ ±33 ms), bildet ikke etter lyden; `SampleTime` på vanlige UVC-kameraer. **Meld tilbake:** forskyvning før/etter per kamera, og hvilken modus loggen viste. `Anchored` med klapp > 1 bilde etter lyden = kameraets faste forsinkelse må inn som konstant per kamera (si fra, tallet står i målingen). **Rødt funn:** bildet systematisk > 2 bilder bak lyden, eller forskyvning som vokser over 2 minutter.
- [ ] **(c3b-win) Videoopptak på Windows går gjennom den native motoren (C3b).** Før C3b gikk et videoopptak på Windows gjennom ffmpeg med kameraet via DirectShow og lyden fra cpal i en pipe (eller DirectShow-lyd under «Klassisk»). Nå tar den native motoren lyden (WASAPI/ASIO) og leser kameraet med Media Foundation, og ffmpeg koder bare bildet (C0a-koderen). **Gjør:** USB-webkamera + USB-mikser, ta opp 5 min med klapp i start og slutt; gjenta med et ASIO-kort; gjenta med pre-roll på og «del opp» satt til 2 min. **Forventet:** én mp4 per del med synkront klapp (±1 bilde), levende bilde i overlegget, kamerakortet sier hva kameraet leverer (det sa «fikk ikke spurt» på Windows før C3b), Avansert har ingen «Opptaksmotor»-rad. Trekk USB-kameraet i 10 s midt i: lyden har ingen hull, bildet fryser/blir svart og kommer tilbake ved neste fragment (gjenkobling av kameraet midt i et fragment er C5). **Rødt funn:** lyd og bilde ute av synk, et opptak som ikke starter, eller hakkete lyd med kamera på (sammenlign CPU med (c0a)).

- [ ] **(a4) Back-offen kuttes når mikseren settes inn igjen (A4).** Start et
      lydopptak på USB-mikseren (eller en USB-mikrofon), la det gå et minutt,
      trekk ut kabelen, vent **ca. 20 sekunder** (så back-off-stigen har nådd
      lange ventetider), og sett den inn igjen. Se på klokka eller i loggen.
      **Forventet FØR A4:** gjenkoblingen skjer først når den pågående
      back-offen er sovet ut — opptil 10 sekunder etter at mikseren er tilbake,
      og opptil 10 sekunder stillhet i opptaket som ingen ba om.
      **Forventet ETTER:** fra første opptak har loggen linja
      `WASAPI endpoint-notification listener installed`, og opptaket kobler seg til igjen
      **innen omtrent ett sekund** etter at mikseren er satt inn (gulvet er
      300 ms, pluss tiden Windows bruker på å gjøre enheten klar). Samme
      opptak fortsetter, ingen feilmelding. **Rødt funn:** ventetiden er
      fortsatt opptil 10 sekunder (lytteren fyrer ikke — sjekk om
      installasjonslinja står i loggen), appen henger eller krasjer når
      mikseren settes inn eller trekkes ut, eller gjenkoblingen går i løkke.
      Gjenta én gang med «standard inndataenhet» som valgt enhet i appen.

- [ ] **(b) Kamera + video, stopp og start rett etter hverandre.** Ta opp en
      videoøkt (kamera + lyd), stopp den, og **innen få sekunder** start et
      helt nytt opptak (f.eks. et kveldsmøte rett etter gudstjenesten).
      **Forventet:** det nye opptaket viser en ren «Tar opp»-tilstand — ingen
      rest av forrige økts «Stoppet»-tilstand vises over det nye. En
      generasjon som ikke er den gjeldende skal ikke få lov til å skrive til
      skjermen.
- [ ] **(c0a) Maskinvarekoding på Windows (C0a, `ombygging/c0a-hw-koder`).**
      Koderne (`h264_nvenc`, `h264_qsv`, `h264_amf`, `h264_mf`) finnes ikke i
      Mac-sidecaren, så hverken prøven eller CPU-gevinsten er kjørt noe sted
      ennå — golden-testene i `capture.rs` og `video_encoder.rs` beviser bare
      argumentene. Dette punktet beviser resten. Bruk en **eldre kirke-PC** om
      du har en (det er den som hakket), ellers Windows-boksen.
      **Rekkefølge:** (1) Mål BASISEN først, med forrige utgivelse (v0.25.3,
      programvare): ta opp **60 minutter** video (1080p30, kamera + lyd) og
      noter CPU-bruk i Oppgavebehandling (gjennomsnitt og topp for
      `sundayrec` + `ffmpeg`, hver 5. min), og antall tapte bilder
      (`drop=` i siste `frame=`-linje i loggen, eller `lost_frames` i
      opptakstelemetrien). (2) Installer denne versjonen, slett
      `%LOCALAPPDATA%\no.sundayrec.app\video-encoder.json` om den finnes, og
      gjenta de 60 minuttene på samme maskin, samme kamera, samme scene.
      **Forventet:** (a) Prøven kjøres i BAKGRUNNEN, ca. ett minutt etter
      appstart, aldri i opptaksstarten: det første videoopptaket etter
      installasjon starter uten ekstra ventetid og bruker `libx264`
      (loggen: «using stored choice (no probe in the start path)»,
      `stored=false`). Etter prøven finnes `video-encoder.json` med første
      koder som virker på maskinen (NVIDIA: `h264_nvenc`, Intel:
      `h264_qsv`, AMD: `h264_amf`, ellers `h264_mf`; `libx264` bare hvis
      ingen virker). Loggen har `video encoder probe` per koder og
      `video encoder: background probe finished`. Start et planlagt
      videoopptak midt i det første minuttet: det skal starte som vanlig
      (programvare), og loggen sier «background probe abandoned» om prøven
      var i gang. Start appen på nytt, og mål (b)–(e) på NESTE opptak. (b) **CPU under halvparten av basisen**
      (snitt over de 60 minuttene) og **ingen tapte bilder** (`drop=0`). (c) Neste
      videoopptak starter UTEN ekstra ventetid (`using stored choice`). (d)
      Filen spiller i VLC og i Windows' egen spiller, har jevn 30 fps, og
      lyd og bilde er i synk ved 60 min (klapp foran kameraet ved start og
      slutt). (e) Bitraten er rimelig: `ffprobe` viser rundt 12 Mbit/s
      (alle fire koderne får `-b:v 12000k`; AMF og MF styrer bitrate etter
      egne regler, så les av faktisk verdi og noter den).
      **Test også:** (f) **Diagnose** (Innstillinger → Avansert) har seksjonen
      «Video encoder» med «In use» og én «Probe»-linje per koder som ble
      prøvd; den kjører prøven på nytt og oppdaterer `video-encoder.json`.
      Under et pågående opptak sier den bare hva som er i bruk («Probe: not
      run»). (g) **Feilende koder:** erstatt innholdet i `video-encoder.json`
      med en koder maskinen IKKE har (f.eks. `h264_nvenc` på en AMD-boks) og
      start et opptak. **Forventet:** opptaket faller til
      `libx264` ved første gjenkobling (loggen sier «falling back to libx264»), filen blir hel, og
      `video-encoder.json` er slettet. NB: dette gjelder ffmpeg-/dshow-stien;
      Windows' vanlige videoøkt (cpal-røret) har ikke gjenkobling, så der
      slutter økta, men valget glemmes og NESTE opptak får `libx264`/ny
      prøve — noter hva som faktisk skjer. (h) Et ikke-NVIDIA-/Intel-/AMD-
      oppsett (Hyper-V-VM, ingen GPU): prøven skal falle til `libx264` uten
      feilmelding og uten mer enn noen sekunders forsinkelse.
      **Hvis CPU-målet ikke nås:** noter koder, GPU, driverversjon og tallene.
      Argumentene per koder (`-preset p4` for NVENC, `-preset veryfast` for QSV,
      `-hw_encoding 1` for MF, kun `-b:v` for AMF) er valgt uten
      maskinvare å prøve dem på — de er det første å justere.
- [ ] **(c0b) Nøkkelbilde hvert 2. sekund med ekte kamera (C0b,
      `ombygging/c0b-nokkelbilder`).** Intervallet er bevist med `ffprobe` på
      et opptak fra `-f lavfi testsrc2` gjennom nøyaktig samme koderargv
      (VideoToolbox og libx264 på Mac: nøkkelbilder ved 0, 2, 4, 6, 8, 10 s),
      men IKKE med et ekte kamera: macOS ga ikke kameratilgang til
      byggemiljøet, og tillatelsen ble ikke omgått. Dette punktet beviser det
      som gjenstår. **Gjør:** (1) På Macen: ta opp 30 s video med det
      innebygde kameraet i appen, og kjør
      `ffprobe -skip_frame nokey` med `-show_entries frame=pts_time` på
      mp4-filen (kommandoen står i SMOKE-TEST).
      **Forventet:** nøkkelbilder ved 0, 2, 4, 6 … s (±ett bilde). (2) Gjenta
      på Windows-boksen med koderen fra (c0a) (NVENC/QSV/AMF/MF) og med
      `libx264` (sett `hw_accel` av). **Forventet:** samme intervall. Er det
      lengre mellom nøkkelbildene med én koder (`h264_mf` er den mest
      mistenkelige: den kan ignorere `-g`), noter koderen og målt intervall.
      (3) Et 60 min opptak: filen søkes jevnt i en spiller, og et
      `kill -9` midt i opptaket taper høyst et par sekunder bilde.
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

- [ ] **(w-a3b) Feilklassene i gjenkoblingen på Windows (native WASAPI).**
      Lydopptak (ikke video), WASAPI-enhet. Tre ting: 1) **Trekk mikseren**
      som i (a). **Forventet:** gjenkoblingsstripe (`device_disconnected`),
      samme opptak fortsetter når den kommer tilbake. 2) **Ta enheten i
      eksklusiv modus:** la et annet program (f.eks. en DAW med
      «exclusive mode» på) ta mikseren midt i opptaket. **Forventet:**
      advarsel med koden `device_busy` (ikke `device_disconnected`), og nye
      forsøk på back-off-stigen til enheten er ledig igjen — ingen terminal
      feil før tidsbudsjettet er brukt opp. 3) **Standardenheten byttes:** ta
      opp med lydenheten satt til systemets standard (tomt navn), og bytt
      standard inndata i Windows' lydinnstillinger midt i opptaket.
      **Forventet:** én gjenkobling, og opptaket fortsetter fra **samme**
      mikrofon som før (gjenkoblingen åpner enheten ved navn), ikke den nye
      standarden. I alle tre: loggen `recorder: native capture starting`
      viser samme `rate=` som enheten har i lydinnstillingene (A3b fant at
      cpal 0.18s «foretrekk 48 kHz» ikke gjelder WASAPI — bekreft det).
      Ingen `STATUS_ACCESS_VIOLATION` (se `audio::com_anchor`).

- [ ] **(c1b) Caps kamera-crates finner kameraene (C1b,
      `ombygging/c1b-kamera-windows`).** CI bygger cratene, men har ikke et
      kamera. Koble til det vanlige USB-kameraet, opptakskortet (HDMI/SDI) og
      start OBS' virtuelle kamera. I en klone av repoet:
      `cargo run -p cap-camera-windows --example enumeration_leak -- formats 60 1000`.
      **Forventet:** hver linje viser samme antall enheter (alle tre) og et
      antall formater større enn null, og tiden per runde stiger ikke.
      Følg prosessen i Oppgavebehandling (Detaljer → Tråder/Håndtak) mens den
      går: tallene skal ligge flatt. Gjenta med `mf` og `ds` i stedet for
      `formats`. **Rødt funn:** «No cameras found», et kamera mangler i én av
      modusene, eller tråder/håndtak vokser for hver runde (lekkasjen Cap
      rettet i #2132/#2143 er tilbake). Selve opptaket fra kameraet bevises i
      C1c (`camera_probe`).

- [ ] **(c1c-w) `camera_probe` på Windows-boksen (C1c,
      `ombygging/c1c-kamera-api`).** Windows-koden i `camera::windows_backend`
      er bare bygget og clippy-sjekket i CI; ingen kamerabilder har gått
      gjennom den. Koble til USB-kameraet (og OBS' virtuelle kamera).
      **Gjør:** `cargo run --example camera_probe -- --list`, så
      `cargo run --example camera_probe -- --camera <del av navnet> --seconds 10`
      for USB-kameraet, og gjenta med `--format 1280x720@30` (NV12/YUY2) og
      et MJPEG-format hvis kameraet tilbyr det. **Forventet:** kameraene står
      med riktig type («kamera» / «virtuelt kamera»), formatlista er ikke
      tom og har ingen doble linjer, «backend: Media Foundation» (DirectShow
      bare for enheter MF ikke klarer), første bilde innen ~2 s, «measured
      fps» ved valgt fps, jitter under noen få ms, «lost» 0, kø-drop 0, og
      `raw bytes` = antall bilder × bildestørrelsen. Etter stopp skal
      programmet avslutte uten henging. Kjør proben tre ganger rett etter
      hverandre (start/stopp-syklus), og én gang mens et opptak i SundayRec
      pågår (ingen krasj; kameraet kan være opptatt, og feilen skal da være
      en ren melding). **Rødt funn:** «Media Foundation did not start»,
      henger ved stopp, `STATUS_ACCESS_VIOLATION`, tidsstempler som går
      bakover, eller at lista over kameraer mangler et som `c1b`-sjekken fant.
      Åpne spørsmål det skal avgjøre: går Windows-bildene med NV12 uten
      radfyll (`raw bytes` delt på bildeantall = bredde × høyde × 1,5)? Opptakskort
      finnes ikke og står som åpent.

- [ ] **(B4a-w) Å åpne et langt opptak på Windows-boksen (B4a,
      `ombygging/b4a-en-dekoding`).** Åpningsrunden leser fila én gang og
      deler den ut på tre tråder (selve lesingen, nivåmåleren og
      16 kHz-halvdelen); CI beviser tallene, men ikke tida på en vanlig PC.
      Åpne et opptak på 60–90 minutter i **Redigering**, både en 48 kHz- og en
      44,1 kHz-fil (WAV eller mp3). **Forventet:** bølgeformen kommer på
      omtrent samme tid som en ren dekoding av fila tar (ca. 10 % av
      spilletida eller mindre), «Leter etter prekenen …» er ferdig noen
      sekunder senere, og lyd-steget kommer med sin setning uten egen ventetid.
      Det går bare **én** `ffmpeg.exe` i Oppgavebehandling under åpningen, og
      CPU-bruken ligger på to–tre kjerner, ikke alle. Loggen har linja
      `open round: time spent per half` med sekundene nivåmåleren og
      16 kHz-halvdelen brukte; skriv dem ned. **Rødt funn:** bølgeformen tar
      mer enn dobbelt så lang tid som før (v0.25.3), to `ffmpeg.exe` samtidig,
      «unreadable» på en fil som åpnet før, eller et kanalvarsel («dødt
      venstre») på et opptak som er i orden.

_(w4, w8–w13, w15 hører til andre F2-Windows-funn som løper i egne
runder — skjulte mapper + OneDrive-varsel, m.fl. Fylles inn her når de
respektive PR-ene er merget; se `docs/NEEDS-RICHARD.md` §«Eierbeslutninger fra F2».)_

## (v) Videomotor

Den nye videoøkta (C3a–C3c): WAV + MKV per fragment, lydklokka som sjef,
konformer som fyller hullene. Fire punkter, i denne rekkefølgen, på **både Mac
og Windows**. Mac-halvdelen kan gjøres på Macen med Continuity-kameraet
(iPhone) eller FaceTime-kameraet; Windows-halvdelen, USB-kabelen og
CPU-lasten venter på riggen. **Terminalen må ha kameratilgang** (Systeminnstillinger →
Personvern og sikkerhet → Kamera) for kommandoene under; Claudes egen terminal
har det ikke, så kjør dem i din egen terminalfane.

**Før du begynner (Mac, 1 min):** `cargo run -p sundayrec --example camera_probe -- --camera iphone --seconds 10`
fra repoet. **Forventet:** «frames» rundt 300, null hull i intervallstatistikken.
Gjør det med FaceTime også (`--camera facetime`). Rødt funn her er C1c, ikke
videomotoren.

**Telleren `video.frames_dropped`.** Ingen sjekk under er grønn hvis overlegget
viser merknaden «Kameraet henger litt etter, så noen bilder er hoppet over».
Merknaden er laget for å være rolig, men et rigg-opptak på en frisk maskin skal
ikke vise den. Står den der: noter koder, CPU-bruk og tallet i loggen
(`video track: frames dropped or the encoder failed this fragment`).

- [ ] **(v1) 90 minutter med klapp i start og slutt: synk innenfor ±1 bilde.**
      Slå på «Ta med kamera», sett kameraet foran en klokke eller en skjerm
      med løpende tidskode, og start et opptak. Klapp **tydelig foran kameraet
      og mikrofonen** de første sekundene. La det gå **90 minutter** (en hel
      gudstjeneste). Klapp igjen de siste sekundene, og stopp.
      **Forventet:** én mp4. Åpne i redigereren eller VLC og gå bilde for
      bilde (`,` og `.` i VLC) til klappet: lydtoppen og bildet der hendene
      treffer hverandre er **høyst ett bilde (33 ms) fra hverandre**, i
      begge ender. Videoen er like lang som lyden (`ffprobe` med `-show_entries stream=duration`, innenfor ett bilde). Overlegget viser
      ingen merknad om tapte bilder. Er «behold ren lydfil» på: en egen WAV
      ligger ved siden av.
      **Mac-delen:** kan kjøres med Continuity-kameraet. **Windows-delen:**
      USB-webkamera + USB-mikser (og gjenta med ASIO-kort om det finnes).
      **Rødt funn:** driv som vokser over opptaket (klappet er 1 bilde unna
      ved start og 5 ved slutt), eller bilde som henger etter lyden fra første
      sekund.
- [ ] **(v2) USB-kabelen til kameraet trekkes i 10 sekunder midt i.** Start et
      videoopptak med klapp i starten. Etter ca. 5 minutter: **trekk kabelen
      til kameraet** (på Macen: ta iPhonen ut av Continuity ved å koble fra
      kabelen/slå av Wi-Fi på telefonen; med FaceTime-kameraet kan det ikke
      trekkes, bruk et USB-kamera), vent **10 sekunder**, sett den i igjen.
      La opptaket gå videre i minst 2 minutter til, klapp igjen og stopp.
      **Forventet:** **lyden har ingen hull** (lytt gjennom hele stedet; i
      redigereren er bølgeformen sammenhengende), bildet **fryser** (siste
      bilde står i to sekunder) og blir så **svart** mens kameraet er borte,
      og **kommer tilbake i løpet av noen sekunder etter at kabelen sitter**
      — midt i fragmentet, uten at opptaket deles (C5a; se (v8) for
      kabeltrekket med loggen åpen). Klappet på slutten er fortsatt innenfor
      ±1 bilde. Opptaket stopper ikke og viser ingen feil.
      **Rødt funn:** hull i lyden, et opptak som ender, eller bilde som aldri
      kommer tilbake.
- [ ] **(v3) `kill -9` midt i et videoopptak.** Start et videoopptak, la det
      gå 2 minutter (klapp i starten). Finn prosessen (`pgrep -x sundayrec`
      eller Oppgavebehandling på Windows) og drep den hardt: `kill -9 <pid>`
      (Windows: `taskkill /F /PID <pid>`). Start appen på nytt.
      **Forventet:** appen tilbyr å gjenopprette opptaket (se (w6) og
      (c3a)), og resultatet er en **mp4 med bilde og nesten hele lyden**
      (høyst et par sekunder tapt i halen). Klappet i starten er synkront.
      Spiller i VLC og QuickTime/Windows' spiller. Kjør også den
      automatiserte varianten med ekte kamera (Mac, fra din egen terminal):
      `SUNDAYREC_REAL_CAMERA=FaceTime cargo test -p sundayrec --lib real_camera_killed -- --ignored --nocapture`
      (bytt til `iPhone` for Continuity-kameraet). **Forventet:** grønn, og
      den skriver `kill -9 … wav ~4 s → mp4 video ~3,5 s, audio ~4 s`.
      **Rødt funn:** opptaket forsvinner, mp4-en har lyd uten bilde, eller
      bildet er forskjøvet mot lyden.
- [ ] **(v4) CPU-last som i C0.** Samme oppsett som (e2) (Mac) og (c0a)
      (Windows): belast CPU-en (`yes > /dev/null` i fire til åtte
      terminalvinduer, eller en tung eksport) mens et **60–90 minutters**
      videoopptak går. Mål CPU i Aktivitetsmonitor / Oppgavebehandling
      (`sundayrec` + `ffmpeg`). **Forventet:** lyden har ingen hull, bildet
      henger med (jevn 30 fps i fila: `ffprobe` med `-select_streams v` og
      `-show_entries stream=avg_frame_rate`), og **telleren står på 0**:
      overlegget viser ingen merknad. Hvis merknaden dukker opp: det er
      riktig oppførsel under overbelastning (kameraet droppet bilder, lyden
      er urørt), men noter hvor mye CPU som skulle til og hvor mange bilder
      loggen sier ble sluppet — det er tallet `video.frames_dropped` vil vise
      i feltet. Windows: sammenlign med (c0a)-basisen.
      **Rødt funn:** hakk i lyden under last, eller et opptak som slutter.
- [ ] **(v5) Forhåndsvisningen fra samme kameraøkt (C4a, C4b).** Terminalen må
      ha kameratilgang; kjør i din egen terminalfane, fra repoet:
      `SUNDAYREC_REAL_CAMERA=FaceTime cargo test -p sundayrec --lib real_camera_preview -- --ignored --nocapture`
      (bytt til `iPhone` for Continuity-kameraet).
      Testen åpner kameraet slik Opptak-sida gjør, lar forhåndsvisningen gå i
      8 s, lar videosporet overta samme økt og tar opp 8 s, stopper opptaket
      slik produksjonsstien gjør (kameraet blir stående, siden en side ser
      på), og lar et nytt fragment ta den samme økta (fragmentgrensen).
      **Forventet:** grønn, og en linje `C4a: 4.xx pictures/s before, 4.xx
during; … camera queue_dropped 0, longest gap …`. Begge tallene mellom 3 og
      5, ingen tapte bilder, ingen feilede forhåndsbilder, og lengste opphold
      mellom to bilder under 800 ms over hele løpet (start, opptak, stopp,
      fragmentgrense).
      **Rødt funn:** under 3 b/s (tappen eller JPEG-koden henger), tapte
      bilder i opptaket mens forhåndsvisningen går, kameraet som må åpnes på
      nytt ved opptaksstart eller ved stopp/fragmentgrense, eller et opphold
      over 800 ms.
- [ ] **(v6) Bildet blinker ikke i appen (C4b, med ekte kamera).** Bygg og
      kjør appen fra din egen terminalfane (kameratilgang): `npm run tauri dev`
      i repoet. Skru på «Ta med kamera», velg FaceTime og senere iPhone. 1. **Opptak-sida:** bildet kommer innen 1–2 s og beveger seg (~4 b/s;
      det er et lite bilde, ikke 30 fps). Slå «speilvend» av og på i
      kamerakortet: bildet speilvendes umiddelbart. 2. **Start** et opptak med video. **Forventet:** overlegget kommer opp
      med bildet allerede i rammen. Ingen «Starter kamera…», ingen svart
      ruter, ingen pause. Bildet fortsetter å bevege seg hele opptaket
      (se også lampa ved kameraet: den slukkes aldri og tennes aldri på nytt). 3. **Stopp** (bekreft). **Forventet:** Opptak-sida har bildet med en
      gang, ingen 3 s venting. 4. **Fragmentgrense:** sett `SUNDAYREC_TEST_SPLIT_BYTES` lavt (se (w)) og
      ta opp et par minutter, eller trekk lydkabelen og sett den i igjen
      (gjenoppkobling = nytt fragment). **Forventet:** bildet i overlegget
      stopper ikke ved grensen, og mp4-en har bilde på begge sider. 5. **Speilvending i fila:** ta opp 10 s med «speilvend» på og åpne fila.
      **Forventet:** bildet i fila er speilvendt akkurat som det du så i
      overlegget. 6. **iPhone/Continuity:** dra ut og sett inn igjen kabelen midt i
      visningen (C5a gjør gjenopprettingen: bildet fryser og kommer tilbake
      av seg selv innen noen sekunder uten at siden åpnes på nytt; se (v7) og
      (v8)).
      **Rødt funn:** bildet forsvinner (selv et blink) ved Start, Stopp eller
      fragmentgrense; kameralampa slukkes og tennes under et opptak; bildet i
      overlegget er av et annet kamera enn det som ble tatt opp; speilvendingen
      er motsatt av fila.
- [ ] **(v7) Kameraet kobler seg til igjen på samme id (C5a, ekte kamera, Mac).**
      Terminalen må ha kameratilgang; kjør i din egen terminalfane, fra repoet:
      `SUNDAYREC_REAL_CAMERA=iPhone cargo test -p sundayrec --lib real_camera_comes_back_on_the_same_id -- --ignored --nocapture`
      (bytt til et USB-kamera for kabeltrekk; FaceTime-kameraet kan ikke
      trekkes). Testen åpner kameraet med opptakets egen `open_camera`, skriver
      én linje i sekundet (`live`/`LOST`, bilder per sekund, om ID-en står i
      kameralista) og venter i 120 s (`SUNDAYREC_OUTAGE_WINDOW_SECS`).
      **Mens den går:** slå kameraet AV (iPhone: dra ut USB-kabelen eller lås
      og legg den bort fra Macen; Continuity-kameraet kommer og går også av seg
      selv), vent til linjen sier `LOST for …`, og slå det PÅ igjen.
      **Forventet:** `LOST` innen 1–2 s etter at bildene stopper, `live` igjen
      innen noen få sekunder etter at kameraet er tilbake, med
      `our id listed: true` hele veien, og til slutt
      `RESULT: PASS — the camera came back on the same id (…)`.
      **Rødt funn:** `our id listed: false` etter at kameraet er tilbake (ID-en
      endrer seg ved hver tilkobling — da kjenner gjenkoblingen det aldri igjen,
      og vi må bestemme om navn skal være reserve; skriv ned begge ID-ene), `LOST`
      som aldri kommer (varslene og bildetellingen ser ikke utfallet), eller
      `live` som aldri kommer tilbake selv om ID-en står i lista (loggen
      `camera reconnect: the camera is listed but did not open`).
- [ ] **(v8) USB-kabeltrekk midt i et opptak, Mac og Windows (C5a).** Samme
      oppsett som (v2), men med appen (`npm run tauri dev`, eller en
      beta-bygning) og loggen åpen (standard loggnivå `info` er nok; macOS:
      terminalen du startet fra, Windows: `%LOCALAPPDATA%\…\logs\sundayrec.log`). Start et videoopptak med
      et USB-kamera, la det gå ett minutt, **trekk kabelen**, vent 10 s og
      **sett den inn igjen**, la opptaket gå ett minutt til, og stopp.
      **Forventet:** loggen sier `the camera has dropped out` (linjen
      begynner med `camera reconnect:`) innen et par sekunder etter trekket,
      og `the camera is back` etter at kabelen sitter (samme linje sier at
      økta er åpnet på samme id); **ingen** nytt fragment på grunn av
      kameraet (kameraet har ikke noe med lydfragmentene å gjøre), lyden er
      sammenhengende, og mp4-en har bilde på begge sider av hullet:
      to sekunder frosset og deretter svart til kameraet er tilbake.
      **Windows (c5a-w):** i tillegg til det over skal loggen ved oppstart ha
      `device_watch: camera interface-notification listener installed`, og
      kameraet skal komme tilbake **raskere enn 2 s** etter at kabelen sitter
      (signalet fra `CM_Register_Notification` kutter ventetiden; uten det tar
      det opptil 1 s til for lista leses). **Mac:** loggen har
      `AVFoundation camera connect/disconnect observers installed`.
      **Prøv også:** et annet USB-kamera (annen ID) i samme port mens det første
      er ute — det skal IKKE overta; sett det første tilbake og se at det er
      det som kommer tilbake. Og: trekk kabelen og **stopp opptaket** mens
      kameraet er ute — Stopp skal gå like fort som ellers.
      **Rødt funn:** hull i lyden, et opptak som splittes eller feiler, bilde
      som aldri kommer tilbake, et annet kamera som tas over, Stopp som henger,
      eller på Windows at tilkoblingen tar flere sekunder (lytteren virker ikke).

- [ ] **(v9) Melding og varsel når kameraet er borte (C5b, Continuity-kamera, Mac).**
      Kjøres av eieren: Claudes Bash har ikke kameratilgang. Bruk
      iPhone som Continuity-kamera (eller et USB-kamera) og en bygning med
      C5b (`npm run tauri dev` fra repoet, eller betaen når den er ute). 1. Innstillinger: velg kameraet, og slå på «Varsel på maskinen»-tillatelsen
      for SundayRec i macOS (Systeminnstillinger → Varsler) om den ikke er det. 2. Start et opptak med video. Bildet i overlegget skal stå. 3. **Utfall:** lås iPhonen og legg den bort fra Macen (eller dra ut USB-kabelen).
      **Forventet:** i løpet av noen sekunder står linja «Kameraet falt ut —
      lyden tas fortsatt opp» i overlegget, og tidtakeren og lydmåleren går
      som før. Ingen alarm-banner. 4. **Gjenkobling før 60 s:** ta kameraet tilbake innen ~30 s.
      **Forventet:** linja forsvinner av seg selv når bildet er tilbake;
      **ingen** systemvarsel kom. 5. **Over 60 s:** gjør utfallet om igjen, men denne gangen **bytt til en
      annen app (f.eks. lysbilder) så SundayRec-vinduet ikke er i fokus**, og
      vent. **Forventet:** ved 60 s (ikke før; ta tiden) kommer ETT
      systemvarsel «Kameraet falt ut — lyden tas fortsatt opp». Det kommer
      ikke igjen mens kameraet er borte (vent to minutter til). 6. Ta kameraet tilbake: linja forsvinner. Gjør et tredje utfall over 60 s
      i samme opptak: **Forventet:** nytt varsel (ett per utfall). 7. Stopp opptaket. Filen har lyd hele veien; bildet er svart over hullene.
      (Et utfall mens vinduet er i fokus gir bare meldingen, ikke varsel; går du
      vekk fra vinduet først etter 60 s, kommer varselet da.)
      **Rødt funn:** varsel før 60 s; flere varsler for samme utfall; ingen
      varsel etter 60 s med vinduet ute av fokus; meldingen blir stående etter
      at bildet er tilbake; meldingen kommer ikke i det hele tatt (da leser ikke
      sporet kamerahelsen; sjekk loggen for `camera reconnect:`-linjene fra (v8)).

- [ ] **(v10) «Behold bare prekenen» på en ekte gudstjeneste med video tar
      sekunder, og lyd og bilde er i synk ved hvert kutt (B6, Mac og Windows).**
      Bruk et ekte opptak med video fra den nye motoren (v0.26 eller nyere, så
      nøkkelbildene står hvert 2. sekund), helst 60–90 minutter. 1. Rediger → Klipp: la prekenen stå (auto-valget), eller sett to kutt for
      hånd. Rediger → Lyd: **ingen** profil først. Eksporter med «Ta med video
      (MP4)». **Forventet:** ferdig på sekunder (tid det, og noter filstørrelse
      og maskin), ikke minutter; loggen sier «cut-only video export: the
      picture is copied». Bildekvaliteten er den samme som i opptaket (det er
      de samme bildene). 2. **Synk ved kuttet.** Spill den eksporterte filen i en vanlig spiller
      (QuickTime/VLC) og hopp til prekenens første ord og til siste setning.
      **Forventet:** leppene stemmer med lyden begge steder; ingen hørbar klikk
      i skjøten. Gjør det samme for et kutt midt i prekenen (sett et kutt over
      en sang inni): leppene stemmer før og etter kuttet. 3. **Klappetest (valgfritt, strengest).** Ta opp 2 minutter der du
      klapper hendene foran kameraet hvert 10. sekund, og eksporter med et kutt
      som ikke ligger på et helt 2-sekunds-tall (f.eks. 33 s). Bla bilde for
      bilde i VLC (`e`) over hvert klapp. **Forventet:** lydtoppen og bildet
      der hendene møtes er innenfor ett bilde (33 ms) ved hvert klapp, også de
      etter kuttet. 4. **Starten kan ligge opptil 2 s tidligere enn du satte den.** Det er
      prisen for å kopiere bildet: en kopi kan bare begynne på et nøkkelbilde,
      og appen går alltid tilbake, aldri fram, så ingen ord blir borte. Lyden
      begynner på samme sted som bildet. Vurder om det er greit for en prekens
      første setning (eierspørsmål i PR-en). 5. **Fallback:** eksporter et gammelt opptak (fra før v0.26, nøkkelbilde
      sjelden) og et kutt kortere enn 2 s inni. **Forventet:** eksporten tar
      minutter som før (bildet kodes om), kuttet er nøyaktig der du satte det, og
      loggen sier «video export re-encodes: …» med grunnen.
      **Rødt funn:** bilde og lyd som glir fra hverandre etter et kutt (mål
      forskjellen og hvilket kutt), svart eller frosset bilde i starten av en
      del, en eksport som aldri blir ferdig, eller en kopi som tar like lang tid
      som før (da kopierte den ikke; send loggen).

## Ørene

Lydkjede-funn som verken CI eller en sidecar-måling kan avgjøre alene — de
trenger et ekte øre på ekte opptaksmateriale (ikke bare et lavfi-testsignal).
Kjør disse på Mac- eller Windows-boksen, med hva du har av ekte
gudstjeneste-opptak eller -materiale for hånden.

- [ ] **(i–ii) Mastringen holder løftet — én forsterkning, ikke gain-ridd.**
      Eksporter ÉN ekte gudstjeneste med presetet `speech-clear` fra denne
      grenen, og lytt etter **pumping** (nivået som kryper opp i pausene og
      ned igjen når stemmen kommer). Prøv deretter en fil med harde topper
      (mikrofonhåndtering, en dør).
      **Historikk:** før B1b gikk nivået gjennom ffmpegs `loudnorm`, som
      falt tilbake til en 3-sekunders gain-rider for de fleste prekener, og
      som landet seks LU under målet på 90-minuttersfila.
      **Forventet NÅ (B1b):** nivået ligger stille, ingen pumping: det er
      én lineær forsterkning til presetets mål, målt på det kuttede,
      behandlede signalet. Harde topper tas av en true-peak-begrenser (tak
      −1,0 dBTP med margin); lytt etter at begrenseren ikke er hørbar på
      vanlig tale. Kvitteringen viser nivået fila faktisk har
      (`Nivå: −16 LUFS`); «(begrenset av topper)» står bare når en fil
      likevel havner mer enn 0,3 LU under målet.
- [ ] **(iii) Mikser-default etter dB-fiksen.** Rediger → «Avansert: åpne
      mikseren» på et ekte prekenopptak, la standardverdiene stå (kompressor
      på, terskel −18 dB, makeup 2 dB), eksporter, og les integrert loudness + true peak mot en eksport gjort med v0.17.x. Ta med en runde på
      gate-slideren i bunn (−70 dB).
      **Forventet FØR fiksen:** makeup/limiter/gate ble tolket LINEÆRT i
      stedet for i dB — 2 dB makeup ga i praksis +6 dB, en 0 dBTP-takgrense
      slapp gjennom med +1 dB på kjøpet, og gate-slideren i bunn (−70 dB) var
      i praksis helt åpen (stengte aldri, for noe).
      **Forventet ETTER (F2-C-A, #238):** samme materiale gir omtrent 4 dB
      mindre gain inn i nivåsteget, true peak lander på −1,0 dBTP der den før
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

- [ ] **(B3b) «Tale» renser av seg selv, og sier hva den gjorde.** Tre
      opptak, hvert i **Rediger → Lyd** med standardprofilen «Tale»; trykk
      **Lytt → Før / Etter** på hvert. 1. **Et ekte brumopptak** (et miksebord eller en lysdemper på samme
      krets, gjerne fra en kirke). **Forventet:** under profilkortene står
      «Vi fjernet brum fra strømnettet», og «Etter» har hørbart mindre
      brum uten at stemmen blir tynn. 2. **Et ekte opptak fra et stort kirkerom** (lang etterklang, helst
      over ett sekund). **Forventet:** «Vi dempet ekkoet i rommet», og
      «Etter» er hørbart tørrere og mer forståelig, uten
      «undervannsklang» på stavelsene. Prøv også en bit med orgel eller
      en holdt akkord rett etter tale: akkorden skal ikke dempes. 3. **Et rent opptak** (stille rom, god mikrofon). **Forventet:** at
      «Før» og «Etter» er **hørbart uendret** (annet enn nivået). Står det
      «Vi dempet bakgrunnsstøyen», skal forskjellen være umulig å høre;
      ofte står det bare «Opptaket var rent.».
      Skriv ned hvilke linjer som sto, og om noen av dem var feil: sier den
      «brum» uten brum, eller «ekko» i et tørt rom, er terskelen (RT60 ≥ 1,0 s,
      stilt på syntetiske signaler) for lav. **Kjent svakhet:** musikk med
      talelignende nivåvariasjon (trommer) kan åpne støyporten, så ta også
      en bit med band og lytt etter at en holdt tone ikke krymper.

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

## (w) Varselstatus på Mac

D2: «Hvem får beskjed?» leser macOS' varselinnstilling for SundayRec. Dette gjør
du selv — Claude rører ikke Systeminnstillinger. Ca. 5 min, kun Macen.

Bygg bundelen (uten signering): `npm run tauri build -- --debug --bundles app
--config '{"bundle":{"createUpdaterArtifacts":false}}'`. Appen havner i
`target/debug/bundle/macos/SundayRec.app`. Logglinjen du leter etter er
`notify: the OS says notifications are …` (appens logg, `~/Library/Logs`).

- [ ] **Utenfor bundle.** `npm run tauri dev`, åpne oppsettsiden «Varsler».
      **Forventet:** «vet ikke» (ikke grønt, ikke rødt), appen krasjer ikke.
- [ ] **Varsler PÅ.** Start `SundayRec.app` (dobbeltklikk). Systeminnstillinger →
      Varsler → SundayRec: «Tillat varsler» på, bannerstil Bannere/Varsler.
      Åpne oppsettsiden «Varsler». **Forventet:** grønt «på». Trykk «Send
      testvarsel»: varselet vises.
- [ ] **Varsler AV.** Slå «Tillat varsler» av for SundayRec, gå tilbake til
      appen (vinduet får fokus, kortet leses på nytt). **Forventet:** rødt «av».
      «Send testvarsel» viser ingenting.
- [ ] **Bannerstil «Ingen».** Slå varsler på igjen, men velg bannerstil
      «Ingen». **Forventet:** rødt (vi regner uten banner som ikke varslet).
      Er du uenig, noter det som funn.
- [ ] **Aldri spurt.** (Kun om SundayRec ikke står i lista under Varsler.)
      **Forventet:** «vet ikke» — appen ber ikke selv om tillatelse. Noter om
      testvarselet likevel vises.
- [ ] **Speiler innstillingen pluginens vei?** I «Varsler AV»-trinnet: vises
      testvarselet likevel? Da lyver kortet, og D2 må ikke strykes fra PLAN.md.
      Noter hva som skjedde i hvert trinn.
      **Rødt funn:** kortet og testvarselet er uenige.

## (a) Avspillingsmotoren (B5a)

Motoren som skal erstatte `<audio>` i Rediger (B5b/B5c) finnes, men har ikke noe
skjermbilde ennå. Den kjøres fra terminalen med kjøreren `examples/playback.rs`,
på maskinens standard lydutgang. Ca. 15 min på Macen; samme kommandoer på
Windows-boksen. Hodetelefoner gjør klikk lettere å høre.

Fra repoets rot (bygg én gang, tar et par minutter):

```bash
cargo build --release --example playback
P=target/release/examples/playback
```

Uten `--at` kjører et fast manus: avspilling fra start, hopp til 2 s før første
kutt (kuttet høres 2 s senere), pause ved 8 s og videre ved 9 s, hopp til 2 s
før andre kutt, og hopp INN i et kutt ved 15 s (skal lande på starten av neste
del). Terminalen skriver hvor spillehodet er hvert sekund, hvor lang tid hvert
hopp tok, og til slutt `underruns`, `xruns` og sanntidsfaktoren.

- [ ] **Avspilling og hopp, fila som den er.** Et ekte opptak, to kutt:
      `$P ~/Downloads/2026.01.04.mp3 --cut 600-1500 --cut 1800-1830`
      **Lytt etter:** klikk eller smell ved start, ved hvert hopp, ved pause og
      ved fortsett (skal være en kort, myk inn/ut-toning, 8 ms); hakk eller
      hull i lyden mellom hendelsene; og ved kuttene (ca. 6 s og 13 s ut i
      manuset) at skjøten er en myk overgang uten klikk og uten et hørbart
      «hull» (20 ms konstant-effekt-overgang). **Forventet:** `underruns 0
  xruns 0` på siste linje, og hvert hopp «heard after» godt under 0,3 s
      (målt på Macen: 95–160 ms).
- [ ] **Samme med lydkjeden.** Legg til `--profile speech-clear`, og én gang
      `--profile voice-noisy-room` (den tyngste kjeden). **Lytt etter:** det
      samme, pluss at lyden ikke «pumper» rett etter et hopp (kjeden får 1 s
      forkjøring før hoppet). **Forventet:** `underruns 0`. Sanntidsfaktoren
      målt på Macen (M1 Pro, 2026-10-19): `speech-clear` ca. 16–22×,
      `voice-noisy-room` ca. 9–10× (produsenten jobber i korte støt og
      kjører da på lav klokke; samme kjede i én lang strekk går 36–118×).
- [ ] **En fil med en annen frekvens.** En 44,1 kHz-fil (eller en hvilken som
      helst `.wav`/`.m4a` fra en telefon): `$P <fil> --cut 20-40`.
      **Lytt etter:** riktig tonehøyde og tempo (ikke «Mikke Mus»), ingen
      hakk. **Forventet:** det samme som over; linja `device:` viser enhetens
      frekvens, og kildens står på første linje.
- [ ] **Eget manus.** `--at T:seek=P` (programsekunder), `--at T:src=S`
      (sekunder i fila), `--at T:pause`, `--at T:play`, `--for S`. Prøv mange
      hopp tett: `--at 2:seek=100 --at 2.3:seek=200 --at 2.6:seek=300`.
      **Forventet:** spillehodet ender på 300 s, ingen klikk, ingen henging.
- [ ] **Windows-boksen (Riggen).** Samme tre første punkter. **Rødt funn:**
      `underruns` over 0 i et løp uten hopp, `xruns` som teller mens du ikke
      gjør noe annet, eller et klikk ved hopp/pause.

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
