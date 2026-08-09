# Endringslogg

Merkbare endringer for deg som bruker SundayRec. Eldre utgivelser enn v0.9.0 er
dokumentert i [utgivelsene på GitHub](https://github.com/SundaySuite-app/sundayrec/releases).

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
