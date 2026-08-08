# Personvernerklæring — anonym diagnostikk i SundayRec

## Kort fortalt

SundayRec kan sende oss anonym informasjon om hvordan programmet oppfører seg,
slik at vi kan finne feil og gjøre det bedre.

- Funksjonen er **av som standard**. Du bestemmer selv.
- Vi får aldri vite hvem du er, hvilken menighet du tilhører, eller hva som ble
  sagt eller sunget.
- **Aldri lyd, transkripsjon eller prekentekst.** Dataformatet har ingen plass
  å legge det i — det er ikke noe vi filtrerer bort i etterkant.
- En krasjrapport bærer selve feilmeldingen, og hver rapport bærer tidspunktet
  den gjelder. Det står forklart under, med hva vi gjør for at en feilmelding
  ikke skal røpe noe om deg.
- Du kan når som helst ombestemme deg, og be om å få alt slettet.
- Å svare nei endrer ingenting i hvordan SundayRec fungerer for deg.

Resten av dokumentet forklarer detaljene, og hvorfor du kan etterprøve dem.

---

## Hva denne erklæringen gjelder — og hva den ikke gjelder

Den gjelder **kun** den valgfrie diagnostikk- og bruksstatistikk-funksjonen,
som du finner under **Innstillinger → System**.

Resten av SundayRec sender aldri noe **til Sunday Suite**, uansett hva du
svarer her. Det finnes ett unntak, oppdateringssjekken, og den er beskrevet i
sitt eget avsnitt rett under fordi den ikke styres av dette samtykket.

At appen ellers sender ting over nett, gjør den selvsagt. Du kan laste opp et
opptak til skylagring, sende et varsel til en webhook, sende e-post, laste ned
en transkripsjonsmodell eller strømme direkte. Alt dette går dit **du** har
bestemt, når du har bedt om det, og aldri innom oss. Denne erklæringen handler
ikke om dem.

---

## Oppdateringssjekk — ikke en del av diagnostikken

SundayRec sjekker med jevne mellomrom om det finnes en nyere versjon, mot
Sunday Suites egen server (`updates.sundaysuite.app`). Tidligere versjoner
spurte GitHub direkte om dette; fra og med denne versjonen spør appen oss i
stedet.

Dette skjer uansett hva du har svart på diagnostikk-spørsmålet, fordi en
oppdateringssjekk ikke er diagnostikk.

**Hva forespørselen inneholder:** ingenting. Ingen installasjons-ID, ingen
innstillinger, ingenting om opptakene dine — og ikke engang hvilken versjon
eller hvilket operativsystem du kjører. Appen spør bare «hva er nyeste
versjon?», får det samme svaret som alle andre, og finner selv ut om det er noe
nyere enn den den allerede har. Serveren får altså ikke vite hvem som spurte,
eller hva de hadde fra før.

**Ingen IP-adresse lagres.** Det håndheves på samme måte som for
diagnostikk-tjenesten, siden det er samme underliggende tjener:
nettverksloggingen er slått av for hele tjeneren, og den loggingen tjeneren
selv gjør, godtar kun et fast sett med felt som ikke identifiserer noen. En
IP-adresse har ingen plass å havne i.

**Du kan slå det av.** Under **Innstillinger → System** finnes «Oppdater
automatisk». Slår du den av, tar appen ikke kontakt med serveren — verken ved
oppstart eller den vanlige sjekken hver time. Det ene unntaket er om du selv
trykker «Se etter oppdateringer nå», for da er det du som har bedt om det.

---

## Behandlingsansvarlig og kontakt

**Sunday Suite** er behandlingsansvarlig for dataene som samles inn hvis du
slår på anonym diagnostikk.

Spørsmål om personvern, innsyn eller sletting kan rettes til
**dev@sundaysuite.app**. Du kan også ta kontakt via
[github.com/SundaySuite-app/sundayrec](https://github.com/SundaySuite-app/sundayrec).

---

## Uten at vi vet hvem du er

SundayRec lager en tilfeldig installasjons-ID (en UUID) på din maskin. Den er
**ikke** utledet fra e-post, navn, kirke eller en Sunday-konto, og den knyttes
aldri til noen av delene. Vi kan altså ikke se hvem du er, hvilken menighet du
tilhører, eller koble to installasjoner til samme person.

Vi kaller funksjonen «anonym diagnostikk» fordi det er slik den oppleves: vi
vet ikke hvem du er. Men for å være helt presis er ID-en teknisk sett et
**pseudonym**, ikke ren anonymitet. Den peker ikke på deg, men den holder
rapportene fra én installasjon sammen.

Den presisjonen er verdt å ta med, for det er nettopp derfor du _kan_ få
slettet dataene dine. Var det helt anonymt, ville det ikke finnes noen måte å
finne igjen hva som var ditt — og «slett mine data» hadde vært et tomt løfte.

Du kan bytte ID-en ut med en ny når som helst. Se «Slette dine data» under.

---

## Hva samles inn hvis du sier ja?

### Krasjrapporter

Hva slags feil som skjedde: feiltype og feilmelding, kuttet til de første 200
tegnene, og hvor i SundayRecs **egen kildekode** det skjedde, på formen
`fil.rs:linje:kolonne`.

Filstier fra din maskin sendes aldri. Skulle en dukke opp i en feilmelding,
fjernes den automatisk og erstattes med `<path>`.

### Kvalitetsdata om opptaket

Om et opptak ble levert fullstendig: tapsprosent, varighet, antall avbrudd
eller dropp, og et Pass/Warn/Fail-verdikt med koder for **hvorfor** — for
eksempel «stort hull i lyden» eller «svakt signal».

Aldri selve lyden, og aldri innholdet i opptaket.

Sammen med dette sendes hvilke _tekniske_ innstillinger som var i bruk:
filformat, samplerate-modus, om video var på, hvor mange planlagte opptak du
har satt opp. Aldri klokkeslett, ukedager, kanalnavn eller navnet du har gitt
et opptak.

### Korrigeringene du gjør i redigeringsverktøyet

Når SundayRec gjetter hvor prekenen begynner og slutter, og du flytter på det,
sendes **hvor ofte** du flyttet noe og **omtrent hvor mye** — oppgitt som et
grovt intervall, for eksempel «prekenstarten ble flyttet 30–60 sekunder
tidligere».

Intervallene er med vilje grove. Hensikten er å se mønstre på tvers av mange
opptak — at forslaget for eksempel systematisk kommer litt for sent — ikke å
kunne rekonstruere ett enkelt opptak.

Det som **aldri** sendes: hva prekenen handlet om, hva som ble sagt, når den
fant sted, hva du har kalt opptaket, eller tekst av noe slag.

**Klokkeslett sendes aldri**, heller ikke som en del av en korrigering. Et
klokkeslett sammen med en varighet peker ut én bestemt gudstjeneste i én
bestemt menighet, og da er tallene ikke anonyme lenger.

Grunnen til at vi ber om dette: den automatiske prekengjenkjenningen skal bli
bedre for alle som bruker den, og et menneske som retter opp en dårlig gjetning
er det eneste signalet som forteller den hva som var galt.

### Hvilke automatiske forslag du tar i bruk

Når appen foreslår en tittel, et sammendrag eller kapittelmerker, sendes hvilken
av de tre det gjaldt, og om du beholdt resultatet slik det ble foreslått eller
skrev det om etterpå.

Selve forslaget sendes aldri. Det du eventuelt skrev i stedet, sendes aldri.
Heller ikke transkripsjonen eller prekenen forslaget ble laget fra.

Dette er ikke noe vi filtrerer bort i etterkant: dataformatet har ingen plass
til tekst overhodet — bare til hvilken av de tre typene det var, og hva som
skjedde med den.

Grunnen til at vi spør om dette også: appen skal kunne lære hvilke typer
forslag som faktisk er verdt å tilby, i stedet for at vi gjetter.

### Funksjonsbruk

Navngitte tellere for hvilke funksjoner som brukes, fra en fast, forhåndsdefinert
liste — for eksempel «eksport til MP3» eller «transkripsjon startet».

Kun **antall** ganger. Aldri hva som ble eksportert, transkribert eller
publisert.

### Det som følger med hver rapport

Uansett hvilken av kategoriene over det gjelder, følger fire opplysninger med:
appversjon, operativsystem, prosessorarkitektur og hvilket språk du har valgt i
appen.

De er der for at vi skal kunne se mønstre som «denne feilen rammer flere på
macOS enn på Windows», uten å vite hvem noen av installasjonene tilhører.

---

## Hva samles ALDRI inn

Lyd. Transkripsjoner. Prekentekst. Navn. E-postadresse. Filstier fra din
maskin. Enhetsnavn, som navnet på mikseren eller lydkortet ditt. Kirke- eller
menighetsnavn.

Dette er ikke bare filtrert bort i etterkant — dataformatet har rett og slett
ingen plass å legge det i.

### Om tidspunkter, som er det ene unntaket verdt å forklare

Hver rapport bærer **når** den gjelder, som et unix-tidsstempel. En
krasjrapport sier når krasjet skjedde; en kvalitetsrapport sier når opptaket
ble avsluttet. Det er nødvendig for å kunne se ting som «denne feilen begynte
i forrige uke» framfor bare «denne feilen finnes».

Vi sier dette rett ut fordi det har en konsekvens du bør kjenne: et tidspunkt
sammen med en varighet peker i praksis ut én bestemt gudstjeneste. Vi vet ikke
hvilken menighet det er, og vi har ingen måte å finne det ut på — men vi kan
se at _en_ installasjon tok opp noe på et gitt tidspunkt.

Derfor er korrigeringene dine bygget annerledes: de bærer **ingen** tidspunkt i
det hele tatt, bare avstander innenfor et opptak. Der finnes det ikke noe å
knytte til en dato.

---

## Hvorfor samler vi inn dette?

For å finne og rette feil raskere, særlig krasj og opptak som ikke ble levert
komplett. For å forstå hvilke funksjoner som faktisk brukes, slik at videre
utvikling prioriteres riktig. Og for at den automatiske analysen skal bli bedre
av å bli rettet på.

---

## Rettslig grunnlag

**Samtykke.**

Du blir spurt — første gang i oppstartsveilederen for nye installasjoner, eller
i et engangsspørsmål for installasjoner som allerede er satt opp.

Du kan når som helst trekke samtykket tilbake, eller gi det på nytt, under
**Innstillinger → System**. Å svare nei endrer ingenting i hvordan SundayRec
fungerer.

---

## Hvor lagres dataene, og hvor lenge?

Hos Sunday Suites egen infrastruktur (Cloudflare, med databehandling i EU).

**Rådata — altså enkeltrapporter — slettes automatisk etter 90 dager.** Etter
det finnes kun irreversibelt aggregerte statistikker igjen: tall som ikke lenger
kan kobles til én bestemt installasjon.

IP-adresser lagres aldri, verken midlertidig eller permanent.

---

## Slette dine data

Under **Innstillinger → System → «Slett mine data»** skjer to ting.

**Umiddelbart, lokalt på din maskin:** installasjons-ID-en din byttes ut med en
ny og urelatert, og rapporter som ventet på å bli sendt, tømmes. Fra det
øyeblikket er ingenting maskinen din sender knyttet til den gamle ID-en.

**Så snart maskinen har nett:** SundayRec ber serveren slette alt som ligger der
under den gamle ID-en. Alle enkeltrapporter fjernes — krasjrapporter,
kvalitetsdata, korrigeringer, bruksmål, alt.

Er maskinen offline når du trykker, sendes forespørselen neste gang den er på
nett. Den blir ikke glemt. Forespørselen sendes selv om du har slått av
diagnostikk, nettopp fordi det å be om sletting og det å slutte å bidra er to
forskjellige ønsker.

Det som ikke fjernes, er de aggregerte statistikkene beskrevet over. Men de
inneholder ingen installasjons-ID og består bare av tall som «3 krasj i versjon
0.8.1 på macOS». Det finnes ingen rad der som er _din_, og derfor ingenting å
slette. Det er også nettopp derfor de kan beholdes.

---

## «Vis hva som sendes»

Under **Innstillinger → System** kan du når som helst åpne en forhåndsvisning
av nøyaktig den datapakken SundayRec ville sendt neste gang.

Dette er ikke et eksempel eller en illustrasjon. Det er de faktiske dataene,
slik de faktisk ville blitt sendt.

Har du diagnostikk slått av, viser den i stedet formen på dataene ut fra din
egen lokale historikk. SundayRec noterer nemlig ting som krasj og opptakskvalitet
lokalt på maskinen din uansett — det er slik «Vis logg» og feilsøking fungerer —
men **ingenting av det forlater maskinen** når du har svart nei. Visningen er
der for at du skal kunne se formatet før du bestemmer deg.

---

## Hvis omfanget endres

Hvis en fremtidig versjon begynner å samle inn noe denne erklæringen ikke
allerede dekker, spør vi deg på nytt før noe sendes.

Et tidligere samtykke dekker kun det omfanget du faktisk sa ja til den gangen.

---

_Sist oppdatert: 2026-08-07._
