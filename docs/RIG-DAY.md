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
tilgang til terminal på Mac-en (for `kill -9`); en kopi-vennlig ekte
`sundayrec.sqlite` det er greit å teste mot; nok tid til at maskinen kan
sovne og våkne av seg selv minst én gang.

Kryss av etter hvert. Et punkt som IKKE stemmer med forventet resultat er en
feilrapport, ikke en avkrysning — noter det og fortsett til neste; ikke la
ett rødt punkt stoppe resten av dagen.

## Mac-riggen

- [ ] **(a) Trekk mikseren midt i opptaket.** Start et opptak, la det gå et
      minutt, trekk ut USB-kabelen til mikseren/mikrofonen og **la den stå
      ute i over 60 sekunder** før du kobler den til igjen.
      **Forventet:** opptaksoverlegget blir stående med en
      gjenkoblingsstripe (ikke en feilmelding), og når enheten kobles til
      igjen fortsetter samme opptak — ÉN fil etterpå, ingen splitt. Ingen
      feil-e-post og ingen system-varsel underveis: dette er en advarsel,
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
- [ ] **(d) WAL-sjekk på en ekte database.** Kjør appen mot en KOPI av en
      ekte `sundayrec.sqlite` (ikke en tom testdatabase) og ta opp normalt.
      **Forventet:** `sundayrec.sqlite-wal` og `sundayrec.sqlite-shm` finnes
      ved siden av hoveddatabasen mens appen kjører, og hele den eksisterende
      opptakshistorikken er intakt og lesbar i **Redigering** etterpå — WAL
      har ikke mistet noe som lå der fra før.
- [ ] **(e) Vekketest.** Skru på «Vekk maskinen fra dvale» (gearikonet →
      Avansert), og bruk **«Test vekking om 2 min»** på kortet «Flere tider
      og spesialopptak». La maskinen sovne (eller sovne den selv).
      **Forventet:** maskinen våkner av seg selv rundt to minutter senere,
      uten et administratorpassord-spørsmål (med mindre appen selv har
      advart om at akkurat denne maskinen trenger et).
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
      hjemme på en dag som ellers handler om Mac-riggen.

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
