# Personvernerklæring — anonym diagnostikk i SundayRec

Denne erklæringen gjelder **kun** den valgfrie, anonyme diagnostikk- og
bruksstatistikk-funksjonen i SundayRec. Funksjonen er **av som standard**, og
du velger selv om du vil slå den på — under **Innstillinger → System**.

Resten av SundayRec (opptak, redigering, publisering) sender aldri noe
uansett hva du svarer her. Denne erklæringen handler bare om det ene,
valgfrie unntaket.

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
  kategorien utvides etter hvert med anonyme, aggregerte tall for hvor ofte du
  gjør bestemte typer korrigeringer i redigeringsverktøyet (f.eks. hvor mange
  ganger et automatisk kapittelforslag ble flyttet) — også dette er kun
  tellinger, aldri hva korrigeringen faktisk var.
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
