# Personvernerklæring — anonym diagnostikk i SundayRec

## Kort fortalt

SundayRec kan sende oss anonym informasjon om hvordan programmet oppfører seg,
slik at vi kan finne feil og gjøre det bedre.

- Funksjonen er **av som standard**. Du bestemmer selv.
- Vi får aldri vite hvem du er, hvilken menighet du tilhører, eller hva som ble
  sagt eller sunget.
- **Aldri lyd, og aldri innholdet i et opptak.** For nesten alt vi samler inn er
  det ikke noe vi filtrerer bort i etterkant — dataformatet har ingen plass å
  legge det i. Det finnes ett unntak, og vi sier hva det er: en krasjrapport
  inneholder feilmeldingen fra programmet, som er tekst. Se «Krasjrapporter».
- **Hvert enkelt punkt er tidfestet** — som et tidspunkt i UTC, uten
  tidssonen din. Se «Om tidspunktene» for hva det betyr og ikke betyr.
- Du kan når som helst ombestemme deg, og be om å få alt slettet.
- Å svare nei endrer ingenting i hvordan SundayRec fungerer for deg.

Resten av dokumentet forklarer detaljene, og hvorfor du kan etterprøve dem.

---

## Hva denne erklæringen gjelder — og hva den ikke gjelder

Den gjelder **kun** den valgfrie diagnostikk- og bruksstatistikk-funksjonen,
som du finner under **Oppsett → Avansert → «Del anonym diagnostikk»**.

Resten av SundayRec sender aldri noe **til Sunday Suite** av seg selv, uansett
hva du svarer her. Det finnes tre unntak, og ingen av dem styres av dette
samtykket:

- **Oppdateringssjekken**, som er beskrevet i sitt eget avsnitt rett under.
- **Innlogging med Sunday-konto**, hvis du velger å logge inn. Da går
  innloggingen til vår egen innloggingstjeneste, og den får naturlig nok vite
  hvem du er — det er hele poenget med å logge inn. Det skjer bare når du selv
  ber om det, og en installasjon som aldri logger inn tar aldri kontakt.
  Innloggingen er ikke koblet til diagnostikken: installasjons-ID-en under er
  ikke utledet fra kontoen din, og de to møtes aldri.
- **Varsling om opptak**, hvis du melder deg på den under **Oppsett → «Hvem
  får beskjed hvis noe går galt?»**. Beskrevet i sitt eget kapittel,
  «E-postvarsling», rett etter oppdateringssjekken.

At appen sender e-post når et opptak feiler, gjør den selvsagt — det er hele
poenget med spørsmålet «Hvem får beskjed hvis noe går galt?». Adressen den går
til er alltid **den du selv har valgt**. Har menigheten sin egen e-postserver
satt opp under Avansert, går meldingen dit, rett til mottakeren, og aldri
innom oss. Har den ikke det — det vanlige for en frivillig uten en
IT-avdeling i ryggen — kan du i stedet melde adressen på Sunday Suites egen
varselsending. Da går meldingen **gjennom vår server**, og videre gjennom
**Resend** — leverandøren som gjør selve utsendelsen — før den når deg. Vår
egen server lagrer den ikke; Resend gjør, en periode. «E-postvarsling»-kapitlet
sier nøyaktig hva det innebærer og hvorfor. Resten av denne erklæringen
handler ikke om det.

---

## Oppdateringssjekk — ikke en del av diagnostikken

SundayRec sjekker med jevne mellomrom om det finnes en nyere versjon, mot
Sunday Suites egen server (`updates.sundaysuite.app`). Tidligere versjoner
spurte GitHub direkte om dette; fra og med denne versjonen spør appen oss i
stedet.

Dette skjer uansett hva du har svart på diagnostikk-spørsmålet, fordi en
oppdateringssjekk ikke er diagnostikk.

**Hva forespørselen inneholder:** ingen installasjons-ID, ingenting om opptakene
dine, og ikke engang hvilken versjon eller hvilket operativsystem du kjører.
Appen spør bare «hva er nyeste versjon?», og finner selv ut om svaret er noe
nyere enn det den allerede har. Serveren får altså ikke vite hvem som spurte,
eller hva de hadde fra før.

Én ting følger likevel med, og det er ærligere å si det: **hvilken
oppdateringskanal du står på.** Appen har to — «stable» og «beta» — og kanalen
er en del av adressen den spør på. Serveren ser altså at _noen_ på beta spurte,
uten å se hvem. Står du på stable, som alle gjør med mindre de selv har valgt
noe annet, er du én av alle. Står du på beta, er den gruppen mindre. Utover det
sender forespørselen bare det en hvilken som helst nettforespørsel må sende for
å komme fram.

Selve oppdateringssjekken lagres ikke. Serveren skriver verken en rad, en teller
eller en loggtekst når den svarer på den — nettopp fordi dette er den ene
forespørselen som også kommer fra installasjoner som har takket nei til
diagnostikk.

**Ingen IP-adresse lagres.** Det håndheves på samme måte som for
diagnostikk-tjenesten, siden det er samme underliggende tjener:
nettverksloggingen er slått av for hele tjeneren, og den loggingen tjeneren
selv gjør, godtar kun et fast sett med felt som ikke identifiserer noen. En
IP-adresse har ingen plass å havne i.

**Du kan slå det av.** Under **Oppsett → Avansert → «Oppdateringer»** finnes
«Oppdater automatisk». Slår du den av, tar appen ikke kontakt med serveren — verken ved
oppstart eller den vanlige sjekken hver time. Det ene unntaket er om du selv
trykker «Se etter oppdateringer nå», for da er det du som har bedt om det.

---

## E-postvarsling — heller ikke en del av diagnostikken

Trykker du **«Bekreft e-postadressen»** under **Oppsett → «Hvem får beskjed
hvis noe går galt?»**, melder SundayRec adressen din på en tjeneste som sender
deg e-post når et opptak feiler, når et planlagt opptak ikke ble noe av, og —
bare hvis du selv har slått på den egne bryteren for det — en kvittering når
et planlagt opptak er ferdig. Meldingen sendes fra `varsel@sundaysuite.app`,
gjennom vår tjener `notify.sundaysuite.app`.

Selve utsendelsen gjør ikke tjeneren vår alene. Den bruker **Resend**, en
navngitt e-postleverandør, til å faktisk levere meldingen til innboksen din.
Det er ikke noe vi har gjemt bort — det er verdt å si tydelig, for det er
Resend som til slutt sitter med meldingen en periode. Se hva det betyr rett
under.

Dette skjer uansett hva du har svart på diagnostikk-spørsmålet, av samme grunn
som oppdateringssjekken over: en varslingstjeneste du selv har bedt om, med et
eget dobbelt samtykke (du trykker «Bekreft», vi sender en lenke, du klikker
den), er ikke diagnostikk.

**Det vi lagrer, fra det øyeblikket du bekrefter:**

- **Adressen din**, i klartekst — uten den kan vi ikke sende deg noe.
- **En tilfeldig abonnements-ID**, mintet på din egen maskin i det øyeblikket
  du trykker «Bekreft». Den er **ikke** installasjons-ID-en diagnostikken
  bruker (se «Uten at vi vet hvem du er» lenger ned): de to mintes hver for
  seg, ingen kode kobler dem sammen, og de møtes aldri — et abonnement
  forteller oss ikke hvilken (eventuelt anonym) installasjon det tilhører.
- **Tidspunktene** rundt abonnementet: da det ble opprettet, da du bekreftet
  det, og sist gang det ble brukt.
- **En kortlevd tellerad** som holder styr på hvor mange bekreftelser én
  adresse har bedt om det siste døgnet, til vern mot at noen bomberer en
  fremmed innboks med bekreftelsesmail. Raden bærer et avtrykk (en hash) av
  adressen — ikke adressen selv — og forsvinner når vinduet går ut.

**Det vi selv aldri lagrer:** selve varselteksten. Meldingen — emne, tekst
og HTML — settes sammen på din egen maskin, på ditt eget språk, og skrives
aldri til noen database på vår egen tjener. Den går videre til Resend for
selve utsendelsen.

**Det Resend lagrer, en periode.** Resend er ikke bare et rør — det er
tjenesten som faktisk sender meldingen. De ser mottakeradressen, emnet,
begge kroppsdelene (tekst og HTML) og avmeldingslenkene i headerne, fordi det
er det som skal til for å levere en e-post. De ser **aldri** noe av
diagnostikken: ikke installasjons-ID-en, ikke telemetrien, og ikke koblingen
mellom et abonnement og en (eventuelt anonym) installasjon — den koblingen
finnes ikke utenfor selve adressen.

Resend holder meldingen i sine driftslogger i **inntil 30 dager**, og
lagringen skjer i **USA**. Overføringen dit er dekket av en
databehandleravtale (Article 28-DPA), EUs standardklausuler (SCC) og EU-U.S.
Data Privacy Framework — de samme rammene de fleste EU-selskaper bruker når
de sender data til amerikanske underleverandører. Avslutter vi kontoen hos
Resend, sletter de resten innen 90 dager; sikkerhetskopier lever i inntil 7
dager til. Se
[Resends grenser for lagring](https://resend.com/docs/knowledge-base/account-quotas-and-limits)
og [Resends personvernside](https://resend.com/security/gdpr).

Ingen IP-adresse lagres hos oss, på samme måte og av samme grunn som resten
av denne erklæringen sier om diagnostikken.

**Hvor lenge:** til du melder deg av — eller, om du aldri bekrefter, i inntil
7 dager. En ubekreftet adresse slettes automatisk etter det, uten at du
trenger å gjøre noe.

**Sletting:** **«Meld meg av»** i appen, eller lenken nederst i hver eneste
varsel-e-post du får fra oss. Begge gjør nøyaktig det samme, og ingen av dem
krever at du logger inn noe sted.

Abonnements-ID-en har med andre ord ingenting med diagnostikk-ID-en å gjøre.
Den ene identifiserer en e-postadresse som ba om å bli varslet; den andre
identifiserer en installasjon som sa ja til å hjelpe oss finne feil. Vi kan
ikke, og har ingen grunn til å, koble dem.

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

**Feilmeldingen er fritekst, og det er det eneste stedet i hele datapakken det
finnes fritekst.** Alt annet vi samler inn er tall og faste valg fra lister vi
har skrevet på forhånd. Feilmeldingen er et unntak fordi den må være det: uten
den kan vi ikke se forskjell på to krasj, og da er det ingen grunn til å samle
inn krasjrapporter i det hele tatt.

Meldingene er skrevet av oss, ikke av deg, og de handler om programmets egen
tilstand. Før en melding sendes, går den gjennom flere passeringer som fjerner
filstier og alt som ser ut som et passord eller en nøkkel. En sti som står for
seg selv erstattes i sin helhet med `<path>`.

Men vi lover ikke at dette fanger alt, for det gjør det ikke. En feilmelding
settes sammen av programmet mens den skrives, og en sti som står inne i en
lengre tekst blir ikke alltid kjent igjen som en sti. Da kan brukernavnet ditt
være borte samtidig som et mappenavn eller et filnavn står igjen. På samme måte
kan et enhetsnavn stå i en melding uten å ligne på noe filtrene leter etter.

Vi nevner det fordi det er forskjell på «dette kan ikke skje» og «dette prøver
vi å hindre», og bare det første er en garanti. For feilmeldinger er det det
andre som gjelder. Rammene rundt er at meldingen kuttes ved 200 tegn, at den
bare handler om en feil, og at rådataene uansett slettes etter 90 dager.

### Kvalitetsdata om opptaket

Om et opptak ble levert fullstendig: tapsprosent, varighet, antall avbrudd
eller dropp, og et Pass/Warn/Fail-verdikt med koder for **hvorfor** — for
eksempel «stort hull i lyden» eller «svakt signal».

Aldri selve lyden, og aldri innholdet i opptaket.

Sammen med dette sendes hvilke _tekniske_ innstillinger som var i bruk:
filformat, samplerate-modus, om video var på, hvor mange planlagte opptak du
har satt opp. Aldri ukedager, kanalnavn eller navnet du har gitt et opptak.

Selve tidspunktet opptaket ble avsluttet følger med, sammen med varigheten. Se
«Om tidspunktene» for hva vi mener om det.

### Korrigeringene du gjør i redigeringsverktøyet

Det finnes to måter å rette appens gjetning på, og begge telles. Den ene er å
**flytte på grensene** — appen fant riktig del av opptaket, men begynte eller
sluttet litt feil. Den andre er å **velge en annen del av opptaket** — appen
trodde noe annet var prekenen, for eksempel et leseinnslag eller en sang, og du
pekte på den riktige blokka. De to rapporteres hver for seg, fordi de forteller
oss to ulike ting om hva som gikk galt.

For begge sendes det samme: **hvor ofte** det skjedde og **omtrent hvor mye**
grensen flyttet seg — oppgitt som et grovt intervall, for eksempel
«prekenstarten ble flyttet 30–60 sekunder tidligere».

Intervallene er med vilje grove. Hensikten er å se mønstre på tvers av mange
opptak — at forslaget for eksempel systematisk kommer litt for sent — ikke å
kunne rekonstruere ett enkelt opptak.

Det som **aldri** sendes: hva prekenen handlet om, hva som ble sagt, når den
fant sted, hva du har kalt opptaket, eller tekst av noe slag.

En korrigering er den ene tingen vi samler inn som **ikke er tidfestet i det
hele tatt**. Den består av tre valg fra faste lister — hvilket punkt, hvilken
retning, hvilket intervall — og et antall. Det er med vilje: en korrigering
forteller oss noe om gjetningen vår, og trenger ikke å si når den ble gjort.

Grunnen til at vi ber om dette: den automatiske prekengjenkjenningen skal bli
bedre for alle som bruker den, og et menneske som retter opp en dårlig gjetning
er det eneste signalet som forteller den hva som var galt.

### Funksjonsbruk

Navngitte tellere for hvilke funksjoner som brukes, fra en fast, forhåndsdefinert
liste — for eksempel «eksport til MP3» eller «opptak startet».

Kun **antall** ganger. Aldri hva som ble eksportert eller tatt opp.

### Resultatet av en diagnose du selv har kjørt

Kjører du **Kjør diagnose** i appen, sendes hvilke funn den kom fram til — som
en fast kode og et alvorlighetsnivå, for eksempel `SR-AUDIO-02` og «advarsel».

Kun koden. Aldri forklaringen som hører til, for det er der detaljene ligger:
hvilken enhet som manglet, hvilken mappe som var full, hvor mye plass som var
igjen. Koden alene er nok til å telle hvor ofte hver situasjon oppstår, og det
er det eneste spørsmålet en anonym statistikk kan svare ærlig på.

En diagnose du kjører mens diagnostikken er slått av, legger ikke igjen noe.

### Planlagte opptak som ikke startet

Var et opptak satt opp til å starte av seg selv, og maskinen ikke våknet eller
ikke kom i gang, sendes at det skjedde: hvilken type svikt det var, en kode for
årsaken — for eksempel `no_resume` — og hvor mange sekunder unna klokka landet
ved en test.

Aldri hva du har kalt det planlagte opptaket, og aldri klokkeslettet det var
satt opp til. At noe gikk galt er nok til å finne feilen; hva menigheten kaller
gudstjenesten sin, og når den begynner, er det ikke.

### Det som følger med hver rapport

Uansett hvilken av kategoriene over det gjelder, følger fire opplysninger om
maskinen med: appversjon, operativsystem, prosessorarkitektur og hvilket språk
du har valgt i appen.

De er der for at vi skal kunne se mønstre som «denne feilen rammer flere på
macOS enn på Windows», uten å vite hvem noen av installasjonene tilhører.

I tillegg følger det med fire opplysninger om selve rapporten: installasjons-ID-
en beskrevet over, tidspunktet pakken ble laget, hvilken utgave av dataformatet
den bruker, og hvilken versjon av dette samtykket den ble samlet inn under. Den
siste er der for at vi skal kunne vise, for hver enkelt rapport, nøyaktig hvilket
omfang du hadde sagt ja til da den ble laget.

### Om tidspunktene

Det står «tidspunkt» flere steder over, og det fortjener et eget avsnitt, for
her har vi tidligere skrevet noe som ikke stemte.

**Hvert enkelt punkt vi samler inn er tidfestet.** En krasjrapport vet når
krasjet skjedde, en kvalitetsrapport vet når opptaket ble avsluttet, et mislykket
planlagt opptak vet når det slo feil, og pakken som helhet vet når den ble laget.
Uten det kan vi ikke se om en feil ble verre etter en oppdatering, og det er en
stor del av grunnen til å samle inn noe som helst.

To ting gjør vi for å begrense hva et slikt tidspunkt sier om deg:

- **Tidssonen din følger ikke med.** Tidspunktet lagres som et punkt i UTC. Der
  appen selv noterer klokkeslett lokalt på maskinen din, står tidssonen i
  teksten; på vei ut faller den bort.
- **Ingenting kobler et tidspunkt til et sted eller et navn.** Vi har verken
  menighetsnavn, IP-adresse, filnavn eller enhetsnavn å knytte det til.

Så la oss være tydelige på hva det ikke betyr. En kvalitetsrapport inneholder
både når et opptak sluttet og hvor lenge det varte, og av det følger når det
begynte. Vet man i tillegg omtrent hvilket land en installasjon står i — og
språkvalget antyder det — er man nærmere «en gudstjeneste et sted» enn tallene
alene skulle tilsi.

Vi mener det er riktig avveining, og vi mener den tåler å bli sagt høyt heller
enn å bli beskrevet som noe den ikke er. Rådata slettes uansett etter 90 dager,
og etter det står bare dagstall igjen.

---

## Hva samles ALDRI inn i diagnostikken

Lyd. Transkripsjoner. Prekentekst. Navn. E-postadresse. Kirke- eller
menighetsnavn. Navnet du har gitt et opptak. Navnet på mikseren eller lydkortet
ditt. Mappen du lagrer i. E-postoppsett. Navnene på de planlagte opptakene
dine, og klokkeslettene de er satt opp til.

For alt dette **i diagnostikk-pakken** er det ikke bare filtrert bort i
etterkant — dataformatet har rett og slett ingen plass å legge det i. Hvert
felt som forlater maskinen i en diagnostikkrapport er enten et tall, et valg
fra en liste vi har skrevet på forhånd, eller en tekst som må gjennom én
bestemt vask først. Det finnes ingen fjerde mulighet, og en utvikler som
legger til et felt uten å plassere det i en av de tre, får en feilende test i
stedet for et smutthull. Denne strukturgarantien gjelder **diagnostikkformatet
spesifikt** — den sier ikke noe om andre ting SundayRec sender, som en
varsel-e-post. Se «E-postvarsling».

Fire presiseringer hører med:

- **Feilmeldingen i en krasjrapport er fritekst.** Den er den ene teksten som
  sendes i diagnostikken, og for den er «ingen plass å legge det i» ikke
  argumentet — vasken er. Se «Krasjrapporter».
- **Tidspunkter sendes.** Ikke som lokale klokkeslett, men som punkter i UTC. Se
  «Om tidspunktene».
- **E-postadressen din lagres — men først når DU melder deg på varsling.**
  Diagnostikken over samler den aldri inn. Melder du deg på Sunday Suites
  varselsending, lagrer vi adressen for å kunne sende deg noe, og den
  slettes igjen når du melder deg av. Se «E-postvarsling».
- **Kirke- eller menighetsnavn, navnet på den ansvarlige, og filnavnet i en
  kvittering kan stå i selve varsel-e-posten.** Ingen av dem er noensinne en
  del av diagnostikken, men en e-post er fritekst av natur: innholdet
  passerer vår egen server kryptert og skrives aldri til noen database der.
  Det passerer også Resend, tjenesten som faktisk sender den videre til deg,
  og som holder den i sine driftslogger i inntil 30 dager. Se
  «E-postvarsling» for hva det innebærer.

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
**Oppsett → Avansert → «Del anonym diagnostikk»**. Å svare nei endrer ingenting
i hvordan SundayRec fungerer.

---

## Hvor lagres dataene, og hvor lenge?

Hos Sunday Suites egen infrastruktur (Cloudflare, med databehandling i EU).

**Rådata — altså enkeltrapporter — slettes automatisk etter 90 dager.** Etter
det finnes kun irreversibelt aggregerte statistikker igjen: tall som ikke lenger
kan kobles til én bestemt installasjon.

Dette 90-dagersløftet gjelder **diagnostikk-rådataene** beskrevet over.
E-postvarslingens abonnement — adressen din, om den er bekreftet — har sin
egen, kortere regel: den lever til du melder deg av, se «E-postvarsling».

IP-adresser lagres aldri, verken midlertidig eller permanent.

---

## Slette dine data

Under **Oppsett → Avansert → «Slett mine data»** skjer to ting.

**Umiddelbart, lokalt på din maskin:** installasjons-ID-en din byttes ut med en
ny og urelatert, og rapporter som ventet på å bli sendt, tømmes. Fra det
øyeblikket er ingenting maskinen din sender knyttet til den gamle ID-en.

**Så snart maskinen har nett:** SundayRec ber serveren slette alt som ligger der
under den gamle ID-en. Alle enkeltrapporter fjernes — krasjrapporter,
kvalitetsdata, diagnosefunn, planlagte opptak som ikke startet, korrigeringer,
bruksmål, alt. Også den tekniske raden som teller hvor mange
rapporter ID-en har sendt, blir borte.

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

Under **Oppsett → Avansert → «Hva sendes»** kan du når som helst åpne en
forhåndsvisning av nøyaktig den datapakken SundayRec ville sendt neste gang.

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
Det er ikke bare en hensikt, men slik det er bygget: hvert samtykke er merket med
hvilket omfang det gjaldt, og sier appen at omfanget har blitt større, stanser
sendingen av seg selv til du har svart på det nye spørsmålet. Et «nei» blir
aldri til et «ja» på veien.

Omfanget denne erklæringen beskriver, er **versjon 2**.

Omfanget kan også bli **mindre** uten at vi spør på nytt — et samtykke til mer
dekker mindre. Det har skjedd én gang: fram til v0.15 kunne appen også
rapportere hvilke typer automatiske forslag (tittel, sammendrag, kapittelmerker)
du tok i bruk. Den funksjonen finnes ikke lenger i SundayRec, og feltet sendes
ikke. Har du sagt ja før, gjelder svaret ditt fortsatt — for det som er igjen.

---

_Sist oppdatert: 2026-09-02._
