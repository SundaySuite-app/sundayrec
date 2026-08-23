# Endringslogg

Merkbare endringer for deg som bruker SundayRec. Eldre utgivelser enn v0.9.0 er
dokumentert i [utgivelsene på GitHub](https://github.com/SundaySuite-app/sundayrec/releases).

## Upublisert

### SundayRec har fått nytt utseende — tre steder: Opptak · Bibliotek · Oppsett

Dette er den største endringen appen har hatt. Alt SundayRec gjør, gjør den
fortsatt; det er _å finne fram_ som er nytt.

Før var det fem sider, åtte faner og 65 innstillinger, tegnet for noen som
allerede visste hvordan appen virket. Nå er det **tre steder**, og de heter det
de er:

- **Opptak** — der du tar opp. Én knapp. Over den står det hva lyden kommer fra
  og om vi faktisk hører den; under står det hva som skjer videre.
- **Bibliotek** — der opptakene ligger. Hver rad heter når den ble tatt opp
  («Søndag 16. august 2026 · 11:00»), ikke hva fila heter. Sletter du noe, får
  du «Angre», og det du sletter havner i en papirkurv som alltid er der.
- **Oppsett** — fem spørsmål, ikke 65 brytere: _Hvilken lyd? Hvor skal
  opptakene? Hvilken kvalitet? Hvilken kirke? Hvem får beskjed hvis noe går
  galt?_ Svar én gang, så er dere klare hver søndag. Alt de fleste aldri trenger
  å røre er samlet under **Avansert**, med en trygg standard.

Og **Rediger**, som ikke er et sted man går, men noe et opptak åpner seg i: tre
steg — **Klipp → Lyd → Eksporter**. Steg 1 åpner med det eneste spørsmålet man
har: _er dette prekenen?_ Forslaget står der allerede, og «Behold bare prekenen»
er ett klikk.

**Det som er annerledes, og hvorfor:**

- **Ingen knapp lyver.** En knapp som er av, sier hvorfor den er av. «Start
  opptak» var grå fordi ingen lydkilde var valgt, og det sto ingen steder.
- **Ingenting sier «alt er i orden» når det ikke er det.** Enhetskortet malte
  «Tilkoblet ✓» for en innstilling ingen hadde satt. Nå står det gult og sier
  hva som mangler.
- **Ingen knapp gjør ingenting.** Finnes ikke skjermen ennå, finnes ikke
  knappen heller. En død knapp lærer bort at knappene i denne appen ikke er til
  å stole på, og den lærdommen overlever knappen.
- **Farlige spørsmål er snudd riktig vei.** «Stoppe opptaket?» har «Fortsett å
  ta opp» som standardvalg. Det er trykk-Enter-svaret, og det skal aldri være
  det som avslutter gudstjenesteopptaket.
- **Tall er ærlige.** Et opptak på 20 sekunder står som «Under 1 min», ikke
  «0 min». Et opptak der lengden ikke er kjent står som «—», og sier ingenting.
- **Alt kan leses.** Håndtakene som viser hvor prekenen begynner og slutter er
  ekte knapper som kan flyttes med piltastene, ikke firkanter tegnet på et
  lerret. Farger, kontrast og bevegelse følger det maskinen er stilt inn på.

**Dette er borte, og det er med vilje:** notatet på et opptak vises, men kan
ikke lenger redigeres · filterbrikkene i historikken (søket gjør jobben) ·
månedskalenderen (faste tider og spesialopptak er to lister under Avansert) ·
eksportvinduet (eksport er et steg) · Diagnose-skjermen · det levende
kamerabildet under opptak. Det som ikke ble bygget på nytt, er skrevet ned —
ikke glemt.

**Språk:** appen er på norsk og engelsk. Svensk, dansk, tysk, fransk og polsk
kommer tilbake i en egen oversettelsesrunde — vi ryddet først bort 653
tekststrenger som ikke lenger vises noe sted, så oversetterne slipper å bruke
tid på skjermer som ikke finnes.

### Avslutt midt i et opptak spør nå én gang til, og venter til fila er trygg

Trykket du Cmd+Q eller «Avslutt» mens gudstjenesten ble tatt opp, avsluttet
SundayRec på flekken. Nå gjør den ikke det: første trykk avslutter ingenting, og
et varsel forteller at det tas opp og at du kan trykke Avslutt igjen innen ti
sekunder hvis du virkelig mener det.

Gjør du det, stoppes opptaket ryddig — og appen blir stående til fila er ferdig
skrevet, i stedet for å forsvinne midt i lagringen. Har du alt trykket Stopp og
opptaket lagres, spør ikke appen på nytt; den venter til fila er trygg og
avslutter så av seg selv. Må du ut med én gang uansett, avslutter et nytt trykk
umiddelbart.

Uten opptak avslutter Avslutt på første trykk, akkurat som før. (Dette erstatter
setningen under v0.15.1-beta.1 om at Avslutt-valget «stopper fortsatt opptaket
med vilje».)

På macOS var Avslutt dessuten helt uavskjærbart før nå: valget gikk utenom
appens egen avslutningsvei og drepte prosessen uten å stoppe opptaket i det hele
tatt. Menylinja øverst på skjermen er bygget om for å lukke det hullet — den ser
lik ut som før.

### Oppdateringens «Start på nytt» venter også på opptaket ditt

Vernet over dekket Cmd+Q og «Avslutt» — men ikke oppdateringen. Trykket du
«Start på nytt og installer» mens gudstjenesten ble tatt opp, stoppet SundayRec
opptaket og byttet ut seg selv med én gang, midt i lagringen. Fila kunne gå tapt
på nøyaktig samme måte som før.

Nå stopper omstarten opptaket ryddig og BLIR STÅENDE til fila er ferdig skrevet
— historikkraden og leveransefila skal finnes — før den nye versjonen starter.
Er du alt i gang med å avslutte, står omstarten over: oppdateringen ligger klar
på disken og tas i bruk neste gang du starter appen.

### Vekking fra dvale respekterer «Ta opp automatisk»

Slo du av «Ta opp automatisk» men beholdt søndagstidene, vekket maskinen seg
likevel 10:50 på søndag for et opptak appen så nekter å ta. Og
vekkingskontrollen meldte de avbestilte vekkingene som «mangler», altså at noe
var galt med maskinen. Begge deler er rettet: bryteren av betyr ingen vekking,
og ingen forventning om en.

Slår vekkingen feil i bakgrunnen — typisk fordi macOS krever administrator for å
skrive en strømhendelse — står det nå i loggen én gang per oppstart, i stedet
for ingen steder.

## v0.15.1-beta.1

Beta-ringens oppfriskning etter v0.15.0 — samme app, pluss tre endringer i
opptaks-ryggraden som fortjener en runde i ringen før de når alle.

### Å lukke vinduet stopper ikke lenger opptaket

Lukket du vinduet mens gudstjenesten ble tatt opp, stoppet opptaket. Nå skjules
vinduet i stedet: opptaket går videre, SundayRec blir stående i menylinja
(systemstatusfeltet på Windows), og et varsel forteller deg hvor du finner
vinduet igjen. Du henter det tilbake fra menylinja, fra Dock-ikonet på macOS,
eller ved å starte SundayRec på nytt.

Det samme gjelder mens opptaket lagres etter at du har stoppet — akkurat der
kunne en lukking før ødelegge en ellers ferdig fil.

Er det ingen opptak i gang, avslutter lukkeknappen appen som før. Avslutt-valget
(Cmd+Q / «Avslutt» i menylinja) stopper fortsatt opptaket med vilje.

### «Ta opp automatisk» kan slås av uten å miste tidene

Å slå av automatisk opptak sletter ikke lenger den ukentlige tidsplanen —
tidene blir stående og venter til bryteren slås på igjen. Planlagte
spesialopptak (enkeltdatoer) går som før uansett.

### Forhåndsbufferen er på fra start

Nye installasjoner får 15 sekunders forhåndsbuffer — lyd fra like før du
trykket Start blir med i opptaket. Har du allerede valgt en verdi (også 0),
røres den ikke.

## v0.15.0 — SundayRec gjør fire ting

Tar opp gudstjenesten, lar deg redigere opptaket, mikser/mastrer lyden og
eksporterer fila. Denne utgivelsen tar ut alt som ikke tjener de fire — både
fra skjermen og fra koden (som ligger i git-historikken om noen trenger den
igjen). Det er første steg i en større ombygging for frivillige som aldri har
sett appen før; selve det nye utseendet kommer i senere utgivelser. Appen ser
altså ut som før, men har færre knapper, færre innstillinger og trenger ikke
lenger en C/C++-kompilator for å bygges.

Gamle innstillinger og eksporterte profiler leses trygt — feltene som hørte til
det fjernede droppes stille, alt annet beholdes.

### Delingsfunksjonene er ute

- **Sky-backup** (Google Drive / Dropbox / OneDrive) og kortet på Hjem.
- **Podkast-feed (RSS)** og hele Podcast-kortet, inkludert «Forhåndsklargjøring
  og gjennomgang».
- **Gjennomgangskøen** — køen på Hjem, påminnelsene, menylinje-varselet og
  redigeringens «klargjort for publisering»-modus. Redigeringen analyserer nå
  ALLTID opptaket når du åpner det.
- **Webhook** til Slack/Discord/Teams.
- **Sunday-suite-koblingene** (SundaySong, SundayPlan, SundayEdit, SundayStage)
  og `sundayrec://`-lenkene.
- **Episodebilde / cover art** (standardbildet og bildet per opptak).
- **Gmail-innlogging** som e-postvei — e-postvarsler fungerer som før, men
  bare via SMTP (vertsnavn, brukernavn og app-passord).

Beholdt: «Send e-post ved feil» med én mottaker og SMTP-oppsettet, og
diagnostikk (med samtykke). Nøkler du hadde lagret for Google eller SundaySong
ligger igjen i maskinens nøkkelring; slett dem der om du vil
(Nøkkelringtilgang → søk «sundayrec»).

### Innholdsfunksjonene er ute

- **Transkribering** (whisper) — «Transkriber»-knappen, modellnedlastingen,
  SRT/VTT/TXT-eksporten, søket i preken-tekst under Historikk og kortet på
  Hjem. Historikk-søket finner fortsatt filnavn, dato og notat. Transkripsjon
  gjøres bedre av verktøy laget for det, og dette var den eneste delen av appen
  som krevde en C/C++-kompilator for å bygge.
- **Prekenhjelp** (AI-oppsummering, tittel og sitater) — panelet i
  redigeringen og nøkkelfeltet under System. Har du lagt inn en
  Anthropic-nøkkel, ligger den igjen i maskinens nøkkelring; slett den der om
  du vil (Nøkkelringtilgang → søk «sundayrec»). Diagnostikken sender ikke
  lenger hvilke forslag du tok i bruk — spørsmålet om samtykke er det samme,
  det dekker nå mindre.
- **Kapittelmerker** — «Legg til kapitler» og kapittellista. Et opptak som
  allerede har kapitler i sidefila beholder dem, men de vises og eksporteres
  ikke lenger.
- **«Hva appen har lagt merke til» og «Hva appen har justert»** under System,
  og bryteren «La appen lære av rettelsene mine». Rettelsene dine i
  redigeringen («Er ikke dette prekenen?») lagres fortsatt ved opptaket og
  telles (med samtykke) i diagnostikken — det er bare visningen og den lokale
  justeringen som er borte.
- **Video-fanen** har nå ett valg: kamera av/på, hvilket kamera, og om du vil
  beholde en separat lydfil. Oppløsning (1080p eller kameraets maks), 30
  bilder/s, MP4/H.264 og maskinvarekoding på Mac er bestemt én gang for alle.
  Den separate lydfila følger lydformatet du har valgt under Filer.
  Video-eksporten i redigeringen prøver alltid maskinvarekoding først på Mac
  og faller tilbake til programvare om den feiler — bryteren er borte.
- **Døde innstillinger** (felter ingenting leste: inngangsvolum, EQ,
  kompressor, limiter, «trim stillhet», «minimer til menylinje» m.fl.) er
  tatt ut av modellen.

### Polsk grammatikk (fra v0.14.1-beta.1)

Beta-ringen fikk v0.14.1-beta.1 den 10. august: setninger som teller to ting
samtidig bøyde bare det ene tallet på polsk. Flaten den fiksa («Hva appen har
justert») er tatt ut over, men bøyingsmotoren består og brukes av de rundt
førti andre setningene som teller noe.

## v0.14.0 — slankere, og stødigere der det gjelder

### Direkte-siden er fjernet — SundayRec er et opptaksprogram

Live-streaming (Direkte-siden, RTMP-destinasjoner med stream-nøkler,
lower-third-overlays) er tatt ut av appen. Funksjonen var aldri riggverifisert,
og kirker som strømmer har allerede dedikerte verktøy til det — SundayRecs jobb
er opptaket som overlever søndagen. Med på lasset gikk NDI-støtten (som aldri
hadde SDK-et sitt), den frakoblede forhåndsvisnings-motoren for kamera (kameraet
i selve opptaks-overlayet er en egen mekanisme og virker som før) og
cue-broens nettverkshalvdel. Gamle innstillinger med stream-felter leses trygt —
feltene droppes stille, alt annet beholdes. Stream-nøkler du har lagret ligger
igjen i maskinens nøkkelring; slett dem der om du vil (Nøkkelringtilgang →
søk «sundayrec»).

### Lisens

SundayRec er nå åpen kildekode under MIT-lisensen.

### Appen gir ikke lenger opp midt i gudstjenesten

Forsvinner lydenheten under et opptak — en usb-kabel som løsner, en mikser som
starter på nytt — prøvde appen å koble seg til igjen tjue ganger og ga så opp.
Det tok rundt tre minutter. Skjedde det under prekenen, var resten av
gudstjenesten tapt.

Nå måles tålmodigheten i tid i stedet for forsøk: de tre første minuttene
oppfører appen seg nøyaktig som før, og deretter fortsetter den å prøve — i
inntil fire timer — samtidig som den sier tydelig fra om hvor lenge enheten har
vært borte. Den slutter aldri stille.

To ting til i samme gate: appen merker med én gang at enheten er tilbake i
stedet for å vente ut pausen sin, og den holder nå fem sekunder med lyd i
minnet i stedet for ett — så et lite hikk i maskinen ikke koster deg sekunder
av opptaket.

### Riktige tallformer på alle språk

«2 opptak» og «1 opptak» ble før valgt med en enkel regel som bare stemmer på
norsk. For polske brukere ga det grammatisk gale former hele veien; tysk, fransk
og de andre manglet entallsformer flere steder. Nå velges formen etter hvert
språks egne regler.

### Under panseret

- **Oppdateringssjekken** kunne tilby deg en oppdatering til versjonen du
  allerede kjørte. Sammenligningen er nå overlatt til et bredt brukt bibliotek
  i stedet for egen kode.
- **Appen ser tydeligere at et opptak lever.** Signalet den bruker til å avgjøre
  «kom opptaket i gang» og «vokser filen fortsatt» kommer nå fra en maskinlesbar
  kanal i stedet for tekst ment for mennesker — teksten endret seg mellom to
  ffmpeg-versjoner, og et friskt opptak kunne da se dødt ut.
- **Planlagt vekking** av maskinen er skrevet om på Windows og leser nå status
  gjennom systemets eget grensesnitt på macOS. Merk: en test-vekking erstatter
  den planlagte — sett tidsplanen på nytt etterpå.

## v0.13.0 — ryddesjauen

Én dag etter v0.12.0, og hele utgivelsen handler om å gjøre appen ærligere:
fjerne det som ikke virket, få det som så ut som det virket til å faktisk
virke, og luke ut tekst som pekte feil vei.

### Innstillingene har fått ett hjem

All lagring av innstillinger går nå ett sted, i stedet for to halvveis synkroniserte.
Det var todelingen som lå bak feilene i v0.12.0 («automatisk sletting virket
aldri», «kanalbytte ble ikke lagret») — nå er selve årsaken borte, ikke bare
symptomene. Første gang du starter denne versjonen flyttes innstillingene dine
over automatisk; du skal ikke merke noe.

### Brytere som nå gjør det de sier

- **«Varsle når opptak starter/stopper» virker** — de to bryterne lagret valget
  ditt og gjorde ingenting. Nå styrer de faktisk varslene. Feilvarsler kan
  aldri slås av: går et opptak galt, får du beskjed uansett.
- **«Vis vindu ved oppstart» er fjernet** — den gjorde aldri noe, og har ikke
  gjort det på lenge. Borte er også en håndfull andre døde valg og knapper som
  lovte ting appen ikke kunne holde, blant dem en YouTube-kobling som alltid
  feilet med en tom feilmelding.

### Mindre rot, riktigere tekst

- Bytter du språk midt i økta, beholder skjermen nå det den holdt på med, i
  stedet for å nullstille status-tekster til standardverdier.
- Integrasjoner-panelet finnes nå på alle sju språk.
- Flere tekster som pekte til faner eller knapper som ikke finnes, er rettet.
- Sletteknapper ser nå ut som sletteknapper.
- Opprydding av gamle opptak og papirkurven jobber nå garantert i opptaksmappen
  din — en intern uenighet om hvor den lå, kunne før la dem lete i feil mappe.
- Har du oppgradert helt fra den gamle utgaven av SundayRec, kan appen nå finne
  igjen de gamle programfilene dens forgjenger la igjen — de ryddes aldri uten
  at du sier ja.

---

## v0.12.0 — den store kvalitetsutgivelsen

Dette er den første vanlige utgivelsen siden v0.10.0, og den samler seks ukers
kvalitetsarbeid: alt fra beta-rundene v0.11.x, pluss en natt med feilretting som
fant ting ingen hadde merket. Kommer du fra v0.10.0 er alt under nytt for deg —
overskriftene fra betaene står lenger ned og gjelder fortsatt.

### Rettet: innstillinger som ikke ble tatt på alvor

Ni innstillinger ble vist og bekreftet i appen uten at motoren noen gang fikk
beskjed. De viktigste:

- **Automatisk sletting av gamle opptak virket ikke** — uansett hva du valgte,
  ble ingenting slettet automatisk. Nå følger motoren valget ditt. Sjekk
  gjerne verdien under **Innstillinger** før søndag, siden den nå betyr noe.
- **Bytte av oppdateringskanal ble ikke lagret.** Du kunne trykke «Ja, bruk
  beta» og forbli på stabil uten å få vite det. Nå lagres valget, og
  tekstlinjen under velgeren forteller hvilken kanal maskinen faktisk henter
  fra.
- Påminnelses-forsprang, inngangsvolum og «lær av rettelsene mine» nådde
  heller ikke fram. Alle gjør det nå, og en automatisk vakt hindrer at nye
  innstillinger kan havne i samme felle.

### Rettet: Integrasjoner-panelet sa «Lagret ✓» uten å lagre

Hele Integrasjoner-panelet kvitterte suksess mens ingenting ble tatt vare på.
Nå er elleve av funksjonene koblet til ordentlig lagring, og de som ennå ikke
finnes bak panelet sier det ærlig i stedet for å late som.

### Rettet: filnavn kunne havne i krasjrapporter

Feilmeldinger som nevnte filer med mellomrom i navnet («gudstjeneste 9. november.wav») kunne slippe deler av navnet gjennom vaskingen som anonym
diagnostikk går gjennom. Meldinger fødes nå rene ved kilden, og en automatisk
vakt passer på at nye feilmeldinger ikke kan gjøre samme feil.

### Raskere å jobbe med

En byggefeil gjorde at deler av appen ble bygget på nytt hver eneste gang,
uansett om noe var endret. Utviklingssyklusen er en femtedel raskere og
kvalitetskontrollen i skyen omtrent dobbelt så rask — noe som betyr at
rettelser når deg raskere.

---

## v0.11.1-beta.2 — appen begynner å lære av deg

> **Betaversjon.** Du får den fordi du står på beta-kanalen under
> **Innstillinger → System**. Bytt til «stable» der hvis du vil tilbake til
> vanlige utgivelser.

Denne versjonen handler om to ting: at appen kan bli bedre av å bli rettet på,
og at du kan se og styre nøyaktig hva den deler.

### Oppdateringer i to ringer, med nødbrems

Oppdateringer kommer nå fra Sunday Suites egen server i stedet for GitHub, og
det finnes to kanaler: «stable» og «beta». Du velger selv under
**Innstillinger → System**.

Det praktiske: en dårlig versjon kan stoppes for alle innen et minutt, i stedet
for å ligge ute til noen rekker å gjøre noe. Det som allerede er installert
berøres ikke — men ingen flere får den.

### Anonym diagnostikk, som du bestemmer over

Du blir spurt én gang om du vil dele anonym informasjon om hvordan programmet
oppfører seg. **Av som standard.** Svarer du nei, endrer ingenting seg.

Under **Innstillinger → System** kan du se den faktiske datapakken appen ville
sendt — ikke et eksempel, men de virkelige tallene — og be om at alt slettes.
Personvernerklæringen forklarer hva som sendes, hva som aldri sendes, og hvor
grensene faktisk går framfor hvor vi skulle ønske de gikk.

### Rettelsene dine blir husket

Når du flytter på appens gjetning om hvor prekenen begynner og slutter, lagres
det nå ved siden av opptaket. Åpner du filen igjen, står ditt valg — ikke
appens.

Ny visning i **Innstillinger → System** viser hva appen har lagt merke til
lokalt: hvor ofte den har bommet, og om forslaget systematisk kommer for tidlig
eller for sent. Ingenting derfra deles med noen med mindre du har slått på
diagnostikk.

Gjennomgangskøen fylles også automatisk etter en analyse, i stedet for å stå
tom.

### Rettet i denne betaen

Elleve feil funnet i en gjennomgang av forrige beta. De du ville merket:

- **Rettelsen din ble lagret og deretter ignorert.** Du korrigerte prekenvalget,
  alt så ut til å gå bra, og ved gjenåpning valgte editoren en annen blokk
  likevel.
- **«Oppdater automatisk» virket ikke.** Hverken ved oppstart eller når du slo
  den av — appen kontaktet serveren uansett. Nå gjør den ikke det.
- **Slettedialogen fortalte deg at sletting på serversiden ikke fantes.** Den
  fantes. Og en slettingsforespørsel gjort mens du var offline ble aldri
  utført.
- **Ett stille lydklipp kunne tømme hele gjennomgangskøen** — alle beslutninger
  om hva som var publisert eller forkastet, borte uten spor.
- **En beta-maskin kom aldri videre til en ferdig versjon.** Den ville stått
  fast og meldt «du er oppdatert» for alltid.

## v0.10.0 — når noe går galt, får du vite det

v0.9.0 ryddet i det du ser. Denne versjonen handler om det du _ikke_ ser: alle
stedene der appen visste at noe var galt, eller visste hvor lang tid noe kom til
å ta, og lot være å si det. Flere knapper som så ferdige ut hadde ingenting bak
seg — de har det nå.

### Appen sier fra — også når du ikke sitter foran maskinen

- **Feilet et opptak, får du beskjed.** Én varslingsvei er bygget for hele
  appen, og den brukes overalt: du får et varsel fra operativsystemet, en e-post
  hvis du har satt det opp, og et kall til webhooken din hvis du har en. Før
  kunne et opptak dø midt i gudstjenesten uten at noe som helst skjedde.
- **E-post virker.** E-postvarsling var bygget, men var aldri med i noen
  utgivelse — du kunne fylle ut alt og få ingenting. Nå er den med.
  SMTP-passordet lagres i nøkkelringen til maskinen (ikke i en fil), du kan
  velge hvilken avsenderadresse som skal stå på, og **Test e-post**-knappen
  sender en ekte melding.
- **Fem feil som pleide å skje i stillhet, snakker nå:** forhåndsopptaket som
  stopper, sky-opplastingen som feiler eller mister tilgangen, en gjenoppretting
  som ble hoppet over, lydenheten som forsvant før start, og lagringsplassen som
  tar slutt _mens_ det spilles inn.

### Innstillingene dine lagres på ordentlig

- **Kirkenavn, e-post- og webhook-innstillinger nådde aldri fram til motoren.**
  De lå bare i nettleserdelen av appen, mens den delen som faktisk sender
  varsler leste standardverdier. Alt du hadde fylt ut så riktig ut på skjermen
  og var borte for den som trengte det. Rettet — og med det også filnavnene som
  skulle inneholde menighetsnavnet.

### Du kan se hvor lenge det er igjen

- **Fremdrift med tid igjen** på det som tar tid: åpning av store filer,
  analyse, transkribering, eksport, lydforbedring, modellnedlasting og
  oppdatering. Anslaget er forsiktig og går aldri bakover; står det stille, sier
  det «beregner…» i stedet for å love noe.
- **Transkribering teller ekte.** Før hoppet den fra 0 % til ferdig. Nå følger
  den lyden gjennom opptaket.
- **Roligere strek.** Alle fremdriftsindikatorer tegnes mykt i stedet for å
  hoppe i trinn, og **Avbryt** svarer med én gang.

### Redigering er ryddet i tre faner

- **Lyd · Innhold · Klipp-verktøy.** Bølgeformen, tidslinjen og lagre-linjen
  står i ro; resten er delt i tre faner slik at editoren får plass på én skjerm
  uten å rulle. Forslag fra appen vises over fanene med en liten gullprikk på
  den fanen det gjelder, så du ikke går glipp av dem.
- **Tomme paneler roper ikke lenger** — de sier kort hva de er til for.

### Slettede opptak kan angres

- **Papirkurv.** Sletting spør ikke lenger — den bare gjør det, og gir deg en
  **Angre**-knapp i ni sekunder. Opptaket havner i en papirkurv sammen med alt
  som hører til (transkripsjon, kapitler, bilde, videofil), og ryddes først bort
  etter 30 dager. Ingenting forsvinner fra Historikk før du sier det.

### Direktesending og episodebilde

- **Direkte-siden viser ekte tall.** Bildefrekvens, bitrate og tilstand per
  destinasjon oppdateres én gang i sekundet mens du sender. Før var
  statuskortet tomt, «Live»-merket satt fast, og feilmeldinger kom som rå koder.
  Selve sendingen er fortsatt ikke testet på en ekte rigg, og siden sier det.
- **Episodebilde.** Du kan sette et standardbilde for alle opptak og et eget
  bilde per episode. De tre flatene som så ut som de gjorde dette, gjorde
  ingenting før nå.

### Lyd

- **Nivåmålerne viser kanalene som faktisk tas opp.** På et digitalbord kunne
  måleren på Hjem, Direkte og i oppsettet vise kanal 1–2 mens opptaket gikk på
  helt andre kanaler — du kunne godkjenne signalet på kanaler ingen tok opp.
- **Bare én ting eier mikrofonen om gangen.** Grensesnittet åpner ikke lenger
  lydenheter selv i det hele tatt; alle målere deler den samme målingen fra
  motoren.
- **Forhåndsopptak uten ffmpeg.** Bryteren for forhåndsopptak bruker nå den
  samme innebygde lydmotoren som opptaket, med én sammenhengende lydstrøm.
  Skjøten mellom det som ble bufret og det du starter er byte-nøyaktig — det
  hørbare hoppet er borte. Den gamle metoden ligger igjen som nødbryter.

### Motoren under

- **Nyere ffmpeg (8.1.2, fra 6.0).** Nedlastingen er låst til en bestemt
  versjon med sjekksum, og pakkes ut uten hjelpeprogrammer utenfra.
- **Vaktbikkja hører etter igjen.** Nyere ffmpeg endret én bokstav i sin egen
  fremdriftsrapport (`kB` → `KiB`). Appen leser den rapporten for å vite at
  opptaket lever — uten rettelsen ville den vært døv for at et opptak hadde
  stoppet, og oppstartslåsen ville hengt. Fanget og rettet før noen så det.

### Språk

Sju språk holdes i takt, tekst for tekst, gjennom hele runden.

## v0.9.0 — UX-gjennomgang

Denne versjonen retter seg mot den frivillige som skrur på opptaket søndag
morgen og ikke skal måtte lure på om det gikk bra. Ingenting av lydmotoren fra
v0.8.1 er rørt — det er grensesnittet, tilbakemeldingene og ærligheten i det du
ser som er bygget om.

### Du får vite hva som skjer

- **Glipp-varsel.** Hvis et planlagt opptak ikke ble til noe, sier appen det med
  én gang du åpner den — med hvilke opptak det gjaldt, og hvorfor det pleier å
  skje. Før måtte du oppdage det selv i Historikk.
- **Sjekk før start.** Et kort på Hjem forteller om noe står i veien for
  neste planlagte opptak (lydenhet borte, for lite lagringsplass, manglende
  tillatelse til mikrofon eller kamera) mens det ennå er tid til å ordne det.
- **Én sannhet om «neste opptak».** Fem steder i appen regnet ut hvert sitt svar
  på når neste opptak var, og de var ikke alltid enige. Nå er det ett svar.
- **«Fullfører opptak…».** Når du stopper, sier appen at den skriver ferdig fila
  i stedet for å se ut som om ingenting skjer.
- **Tap av data får sitt eget varsel** som blir stående til du har lest det, i
  stedet for å kaste deg til en annen side.
- **Menylinje-ikonet lever.** Ikonet viser om det spilles inn, menyen viser
  gjeldende status, og den følger språkvalget ditt.

### Færre steder å gå feil

- **Fem faner i Innstillinger i stedet for sju**: Lyd · Video · Opptak · Deling
  · System. «Publisering» og «Varsler» var to halvdeler av samme spørsmål — hvem
  som får opptaket etterpå — og er nå seksjoner under Deling.
- **Innstillinger lagres når du endrer dem**, med en «Lagret ✓»-kvittering. Den
  gamle «Lagre»-foten er borte — sammen med en feil der Lagre-knappen på
  Video-siden aldri kunne nås.
- **Ærlige flater.** Funksjoner uten noe bak seg i denne bygningen sier det rett
  ut («Kommer», «Ikke tilgjengelig», «Ikke konfigurert») og er slått av, i
  stedet for å ta imot klikk som ikke gjør noe.
- **Onboarding** har fått tilbake-knapp, og tidsplanen du setter opp der legges
  til i stedet for å overskrive det som allerede fantes.

### Redigering og historikk

- **Historikk** kan sorteres og filtreres, sletting spør først, og lyd- og
  videofil fra samme opptak paret aldri i Tauri-versjonen — det gjør de nå.
- **Preken-søket virker igjen**, sammen med gjennomgangskøen og eksport av
  transkripsjon (også som ren tekst).
- **Editoren** har fått en fastlåst lagre-linje som ikke forsvinner når du
  scroller, synlig resultat etter eksport, og hurtigtastene bak et «?».
- **Roligere bilde.** Zoom med hjulet og avspilling tegner én gang per bilde i
  stedet for å kjempe mot seg selv.

### Detaljene

- Alle systemdialoger («Er du sikker?») er erstattet med appens egne — de kan
  oversettes, de kan avbrytes med Escape, og de fryser ikke vinduet.
- Nivåmålerne oppfører seg likt på 30, 60 og 120 bilder i sekundet.
- Gullknappene har mørk tekst (var hvitt på gull — for lite kontrast),
  fokusringer er synlige, og sidebytte tilbakestiller rullingen.
- Automatisk stopp styres nå av motoren, ikke av grensesnittet.
- **Forhåndsopptak** («preroll») ligger bak en av-som-standard bryter merket
  eksperimentell. Ikke slå den på før du har testet den på riggen din.

### Språk

Sju språk holdes i takt. Et par tekster pekte fortsatt på faner som ikke finnes
lenger, og «Maks»-avlesningen under nivåmålerne var norsk uansett språk — begge
deler er rettet.
