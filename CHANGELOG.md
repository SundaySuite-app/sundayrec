# Endringslogg

Merkbare endringer for deg som bruker SundayRec. Eldre utgivelser enn v0.9.0 er
dokumentert i [utgivelsene på GitHub](https://github.com/SundaySuite-app/sundayrec/releases).

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
