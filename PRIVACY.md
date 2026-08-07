# Personvernerklæring — anonym diagnostikk i SundayRec

Denne erklæringen gjelder **kun** den valgfrie, anonyme diagnostikk- og
bruksstatistikk-funksjonen i SundayRec. Funksjonen er **av som standard**, og
du velger selv om du vil slå den på — under **Innstillinger → System**.

Resten av SundayRec (opptak, redigering, publisering) sender aldri noe
uansett hva du svarer her — **med ett unntak: oppdateringssjekken**, beskrevet
for seg rett under, fordi den ikke er en del av diagnostikk-funksjonen og
ikke styres av samtykket her. Resten av denne erklæringen handler om det
valgfrie, avslått-som-standard diagnostikk-unntaket.

## Oppdateringssjekk — ikke en del av diagnostikk

SundayRec sjekker med jevne mellomrom om det finnes en nyere versjon, mot
**Sunday Suites egen server** (`updates.sundaysuite.app`). Tidligere versjoner
spurte GitHub direkte om dette; fra og med denne versjonen spør appen oss i
stedet.

Dette skjer uansett hva du har svart på diagnostikk-spørsmålet over — en
oppdateringssjekk er ikke diagnostikk. Forespørselen inneholder **ingen
installasjons-ID, ingen innstillinger og ingenting om opptakene dine**. Den
inneholder faktisk ikke engang hvilken versjon eller hvilket operativsystem
du kjører: appen spør bare «hva er nyeste versjon?», får det samme svaret som
alle andre, og finner selv ut om det er noe nyere enn den den allerede har.
Serveren får med andre ord ikke vite hvem som spurte — eller hva de hadde fra
før.

Ingen IP-adresse lagres for denne forespørselen heller. Det håndheves på
samme måte som for diagnostikk-tjenesten (samme underliggende tjener):
nettverksloggingen er slått av for hele tjeneren, og den eneste loggingen
tjeneren selv gjør, godtar kun et fast sett med felt som ikke identifiserer
noen — en IP-adresse har ingen plass å havne i.

Du kan slå av **«Oppdater automatisk»** under **Innstillinger → System**. Da
stopper de automatiske sjekkene helt — appen tar ikke kontakt med serveren
verken ved oppstart eller den vanlige time-for-time-sjekken. Det ene unntaket
er hvis du selv trykker **«Se etter oppdateringer nå»**: den knappen fungerer
uansett, fordi det da er en handling du selv har bedt om, ikke noe appen
gjør bak ryggen din.

## Behandlingsansvarlig

**Sunday Suite** er behandlingsansvarlig for dataene som samles inn hvis du
slår på anonym diagnostikk.

## Fullstendig anonymt

SundayRec lager en tilfeldig installasjons-ID (en UUID) på din maskin. Den er
**ikke** utledet fra e-post, navn, kirke eller en Sunday-konto, og den knyttes
aldri til noen av delene. Du kan bytte den ut med en ny når som helst — se
«Slette dine data» under.

## Hva samles inn hvis du sier ja?

Tre kategorier, og ingenting annet:

- **Krasjrapporter** — hva slags feil som skjedde (feiltype/melding, kuttet
  til de første 200 tegnene), app- og OS-versjon, og en teknisk
  filplassering (`fil.rs:linje:kolonne`) inne i SundayRecs egen kildekode.
  Absolutte filstier fra din maskin sendes aldri — de fjernes automatisk og
  erstattes med `<path>` hvis de skulle dukke opp i en feilmelding.
- **Kvalitetsdata** — om et opptak ble levert fullstendig: tapsprosent,
  varighet, antall avbrudd/dropp, og et Pass/Warn/Fail-verdikt med koder for
  HVORFOR (f.eks. «stort hull i lyden», «svakt signal») — aldri selve lyden
  eller innholdet i opptaket. Sammen med dette sendes en oversikt over hvilke
  _tekniske_ innstillinger som var i bruk (f.eks. filformat, samplerate-modus,
  om video er på, hvor mange planlagte opptak du har satt opp) — aldri
  klokkeslett, dager, kanalnavn eller navnet du har gitt et opptak. Denne
  kategorien utvides etter hvert med anonyme tall for korrigeringene du gjør i
  redigeringsverktøyet: hvor ofte du flytter et automatisk forslag, og omtrent
  hvor mye du flyttet det — oppgitt som et grovt intervall, for eksempel
  «prekenstarten ble flyttet 30–60 sekunder tidligere». Intervallene er med
  vilje grove. Hensikten er å se mønstre på tvers av mange opptak — at
  forslaget for eksempel systematisk kommer litt for sent — ikke å kunne
  rekonstruere ett enkelt opptak. Det som fortsatt aldri sendes, er hva
  prekenen handlet om, hva som ble sagt, når den fant sted, hva du har kalt
  opptaket, eller tekst av noe slag. **Klokkeslett sendes aldri**, heller ikke
  som en del av en korrigering: et klokkeslett sammen med en varighet peker ut
  én bestemt gudstjeneste i én bestemt menighet, og da er ikke tallene anonyme
  lenger. Grunnen til at vi ber om dette, er at den automatiske
  prekengjenkjenningen skal bli bedre for alle som bruker den — og et menneske
  som retter opp en dårlig gjetning er det eneste signalet som forteller den
  hva som var galt.
- **Funksjonsbruk** — navngitte tellere for hvilke funksjoner som brukes, fra
  en fast, forhåndsdefinert liste (f.eks. «eksport til MP3» eller
  «transkripsjon startet»). Kun **antall** ganger — aldri hva som ble
  eksportert, transkribert eller publisert.

I tillegg sendes appversjon, operativsystem, prosessorarkitektur og hvilket
språk du har valgt i appen — slik at Sunday Suite kan se mønstre som «rammer
denne feilen flere på macOS enn Windows», uten å vite hvem noen av
installasjonene tilhører.

## Hva samles ALDRI inn

Lyd, transkripsjoner, prekentekst, navn, e-postadresse, filstier, enhetsnavn
(f.eks. navnet på mikseren eller lydkortet ditt), eller kirke-/
menighetsnavn. Dette er ikke bare filtrert bort i etterkant — dataformatet
har rett og slett ingen plass å legge det i.

## Hvorfor samler vi inn dette?

Diagnostikken hjelper Sunday Suite finne og rette feil raskere — særlig
krasj og opptak som ikke ble levert komplett — og forstå hvilke funksjoner
som faktisk brukes, slik at videre utvikling prioriteres riktig.

## Rettslig grunnlag

**Samtykke.** Du blir spurt — første gang i oppstartsveilederen for nye
installasjoner, eller i et engangsspørsmål for installasjoner som allerede
er satt opp. Du kan når som helst trekke samtykket tilbake, eller gi det på
nytt, under **Innstillinger → System**. Å svare nei endrer ingenting i
hvordan SundayRec fungerer.

## Hvor lagres dataene, og hvor lenge?

Hos Sunday Suites egen infrastruktur (Cloudflare, med databehandling i EU).

**Rådata — enkeltrapporter — slettes automatisk etter 90 dager.** Etter det
finnes kun irreversibelt aggregerte statistikker igjen: tall som ikke lenger
kan kobles til én bestemt installasjon. IP-adresser lagres aldri, verken
midlertidig eller permanent.

## Slette dine data

Under **Innstillinger → System → «Slett mine data»** skjer to ting.

**Umiddelbart, lokalt på din maskin:** installasjons-ID-en din byttes ut med
en ny og urelatert, og rapporter som ventet på å bli sendt, tømmes. Fra det
øyeblikket er ingenting maskinen din sender knyttet til den gamle ID-en.

**Så snart maskinen har nett:** SundayRec ber serveren slette alt som ligger
der under den gamle ID-en. Alle enkeltrapporter fjernes — krasjrapporter,
kvalitetsdata, bruksmål, alt. Er maskinen offline når du trykker, sendes
forespørselen neste gang den er på nett; den blir ikke glemt. Denne
forespørselen sendes selv om du har slått av diagnostikk, nettopp fordi det å
be om sletting og det å slutte å bidra er to forskjellige ønsker.

Det som ikke fjernes, er de aggregerte statistikkene beskrevet over — men de
inneholder ingen installasjons-ID og består bare av tall som «3 krasj i
versjon 0.8.1 på macOS». Det finnes ingen rad der som er _din_, og derfor
ingenting å slette. Det er også nettopp derfor de kan beholdes.

## «Vis hva som sendes»

Under **Innstillinger → System** kan du når som helst åpne en forhåndsvisning
som viser nøyaktig den datapakken SundayRec ville sendt neste gang — eller,
hvis diagnostikk er slått av, formen på dataene fra din egen lokale historikk
(ingenting sendes i så fall — visningen er bare til for å vise deg formatet).
Dette er ikke et eksempel eller en illustrasjon — det er de faktiske,
virkelige dataene, slik de faktisk ville blitt sendt.

## Hvis omfanget endres

Hvis en fremtidig versjon av SundayRec begynner å samle inn en **ny
kategori** av data som denne erklæringen ikke allerede dekker, spør vi deg
på nytt før noe sendes. Et tidligere samtykke dekker kun det omfanget du
faktisk sa ja til den gangen.

## Kontakt

Spørsmål om personvern kan rettes til Sunday Suite via
[github.com/SundaySuite-app/sundayrec](https://github.com/SundaySuite-app/sundayrec).

---

_Sist oppdatert: 2026-08-07._
